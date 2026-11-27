//! The display: a back buffer of 32-bit `0x00RRGGBB` pixels and `flush(rect)`,
//! which puts a rectangle of it on screen. Two backends:
//!
//! - virtio-gpu (`gpu.rs`) when the PCI probe finds one: the back buffer is the
//!   host resource's backing, and a flush transfers and flushes the rectangle;
//! - otherwise the framebuffer Limine got from UEFI GOP, drawn in place, whose
//!   flush does nothing.
//!
//! GOP is set up first, from kmain, before there is a heap; virtio-gpu replaces
//! it once the devices are up. The mode is `display=WIDTHxHEIGHT` when given and
//! in range, otherwise the largest sensible mode the device offers
//! (`gfxcore::mode`).

pub mod claim;
mod gpu;

use crate::log;
use alloc::boxed::Box;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use gfxcore::Rect;
use gfxcore::pixel::Format;
use gpu::Gpu;
use limine::framebuffer::Framebuffer;
use spin::Mutex;

/// A back buffer: `height` rows of `stride` pixels, the first `width` shown.
#[derive(Clone, Copy)]
pub struct Surface {
    pub base: NonNull<u32>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    /// How a pixel is packed: virtio-gpu's back buffer is always `0x00RRGGBB`;
    /// a GOP framebuffer's is what its masks say.
    pub format: Format,
}

// SAFETY: a surface is plain memory that outlives the kernel's use of it; who
// draws where is up to its users (the console, under its own lock).
unsafe impl Send for Surface {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Gop,
    VirtioGpu,
}

enum Backend {
    Gop,
    Gpu(Box<Gpu>),
}

struct Display {
    surface: Surface,
    backend: Backend,
    /// The screen's size in mm from its EDID, (0, 0) when unknown (GOP gives
    /// none, and virtio-gpu's is made up).
    size_mm: (u32, u32),
    /// A GOP framebuffer whose pixels are not `0x00RRGGBB` (RGBX, 10 bits a
    /// channel) cannot be drawn in by its clients: they get this back buffer in
    /// `0x00RRGGBB` instead, and [`present`] packs what they flush into the
    /// framebuffer. Made on the first claim, since GOP is set up before there is
    /// a heap; the console draws on the framebuffer itself, packing as it goes.
    shadow: Option<Surface>,
}

// SAFETY: the device state is used only under the DISPLAY mutex, by one CPU at a
// time; nothing in it is tied to a particular CPU.
unsafe impl Send for Display {}

static DISPLAY: Mutex<Option<Display>> = Mutex::new(None);
/// A flush found the display busy: the next one sends the whole screen.
static BEHIND: AtomicBool = AtomicBool::new(false);

/// The pixel format Limine reports for `fb`: its red, green and blue sizes and
/// shifts. Masks that make no pixel (none reported, or overlapping) leave the
/// usual `0x00RRGGBB`.
fn format_of(fb: &Framebuffer) -> Format {
    Format::from_masks(
        (fb.red_mask_size, fb.red_mask_shift),
        (fb.green_mask_size, fb.green_mask_shift),
        (fb.blue_mask_size, fb.blue_mask_shift),
    )
    .unwrap_or(Format::NATIVE)
}

/// The EDID block Limine passes with `fb`: empty when the firmware gave none.
///
/// `Framebuffer::edid` builds its slice from a null pointer in that case, which
/// a debug build refuses, so the two fields are read from the protocol's layout
/// instead: `edid_size` then `edid`, after the 48 bytes of mode.
pub fn edid_of(fb: &Framebuffer) -> &[u8] {
    const SIZE_AT: usize = 6;
    const _: () = assert!(size_of::<Framebuffer>() == (SIZE_AT + 2) * 8);
    let words = core::ptr::from_ref(fb).cast::<u64>();
    // SAFETY: `Framebuffer` is `repr(C)`, 64 bytes long (checked above), and
    // its `edid_size` and `edid` are the 7th and 8th words, as Limine fills them.
    let (size, edid) = unsafe { (words.add(SIZE_AT).read(), words.add(SIZE_AT + 1).read()) };
    // An EDID is 128 bytes a block, at most 256 blocks: a longer one is not
    // read past that, whatever the size field says.
    const MAX: u64 = 256 * 128;
    match (size, edid) {
        (0, _) | (_, 0) => &[],
        // SAFETY: Limine's EDID is `size` bytes at `edid`, kept for the kernel's
        // life; the first `MAX` of them are no more than that.
        (size, edid) => unsafe {
            core::slice::from_raw_parts(edid as *const u8, size.min(MAX) as usize)
        },
    }
}

/// Takes over a 32-bpp GOP framebuffer. Allocates nothing.
pub fn init_gop(fb: &Framebuffer) -> Option<Surface> {
    let base = NonNull::new(fb.address().cast::<u32>())?;
    if fb.bpp != 32 {
        return None;
    }
    let surface = Surface {
        base,
        width: fb.width as usize,
        height: fb.height as usize,
        stride: (fb.pitch / 4) as usize,
        format: format_of(fb),
    };
    *DISPLAY.lock() = Some(Display {
        surface,
        backend: Backend::Gop,
        size_mm: (0, 0),
        shadow: None,
    });
    Some(surface)
}

/// Brings up a virtio-gpu in the mode `cmdline` asks for or the largest sensible
/// one, and makes it the display. Returns its back buffer, zeroed.
pub fn init_gpu(
    transport: crate::virtio::split::SplitTransport,
    cmdline: &str,
) -> Result<Surface, &'static str> {
    let mut gpu = Gpu::new(transport)?;
    let current = gpu.display_info()?;
    let mut offered = gpu.edid_modes()?;
    offered.extend(current);
    let requested = gfxcore::mode::requested(cmdline);
    let mode = gfxcore::mode::choose(&offered, requested);
    let reason = match requested {
        Some(asked) if asked == mode => "display=",
        Some(_) => "display= out of range; largest offered",
        None => "largest offered",
    };
    log!(
        "display: virtio-gpu offers {} modes, scanout {}; {}x{} ({reason})",
        offered.len(),
        current.map_or_else(
            || alloc::string::String::from("off"),
            |(w, h)| alloc::format!("{w}x{h}")
        ),
        mode.0,
        mode.1
    );
    let base = gpu.set_mode(mode)?;
    let surface = Surface {
        base: NonNull::new(base).ok_or("virtio-gpu: no back buffer")?,
        width: mode.0 as usize,
        height: mode.1 as usize,
        stride: mode.0 as usize,
        format: Format::NATIVE,
    };
    gpu.flush(Rect::screen(mode.0, mode.1))?;
    // A virtio-gpu screen is a window on a host: its EDID's size is made up
    // (QEMU's is its default mode's at a fixed density, whatever the mode),
    // so it is unknown here and the scale goes by the width.
    if let Some((width, height)) = gpu.size_mm() {
        log!("display: the EDID says {width}x{height} mm; a virtual screen's size is unknown");
    }
    *DISPLAY.lock() = Some(Display {
        surface,
        backend: Backend::Gpu(Box::new(gpu)),
        size_mm: (0, 0),
        shadow: None,
    });
    Ok(surface)
}

/// The screen's size in mm from its EDID, width by height; (0, 0) when it is
/// unknown (no EDID, a virtual screen's) or there is no display.
pub fn size_mm() -> (u32, u32) {
    DISPLAY
        .lock()
        .as_ref()
        .map_or((0, 0), |display| display.size_mm)
}

/// The back buffer clients draw in, if there is a display: always `0x00RRGGBB`.
/// On a GOP framebuffer of another format that is a shadow of it, made here the
/// first time ([`present`] packs it onto the screen); None if there are no
/// frames for it.
pub fn surface() -> Option<Surface> {
    let mut guard = DISPLAY.lock();
    let display = guard.as_mut()?;
    if display.surface.format.is_native() {
        return Some(display.surface);
    }
    if display.shadow.is_none() {
        display.shadow = make_shadow(display.surface);
    }
    display.shadow
}

/// A zeroed `0x00RRGGBB` back buffer the size of `screen`, in contiguous frames
/// in the direct map (kept for the kernel's life).
fn make_shadow(screen: Surface) -> Option<Surface> {
    use crate::virtio::hal::KernelHal;
    use virtio_drivers::{BufferDirection, Hal, PAGE_SIZE};
    let pages = (screen.width * screen.height * 4).div_ceil(PAGE_SIZE);
    let (phys, virt) = KernelHal::dma_alloc(pages, BufferDirection::Both);
    if phys == 0 {
        log!("display: no frames for a 0x00RRGGBB back buffer over a non-BGRX GOP");
        return None;
    }
    log!(
        "display: GOP pixels are packed by its masks; clients draw in a {}x{} back buffer",
        screen.width,
        screen.height
    );
    Some(Surface {
        base: virt.cast::<u32>(),
        width: screen.width,
        height: screen.height,
        stride: screen.width,
        format: Format::NATIVE,
    })
}

/// [`flush`] for a client's rectangle of [`surface`]: where that is a shadow of
/// a GOP framebuffer, packs the rectangle onto it. Elsewhere it is `flush`.
pub fn present(rect: Rect) -> Result<(), &'static str> {
    {
        let guard = DISPLAY.lock();
        if let Some(Display {
            surface: screen,
            shadow: Some(shadow),
            ..
        }) = guard.as_ref()
        {
            let rect = rect.clamp(screen.width as u32, screen.height as u32);
            pack_rows(*shadow, *screen, rect);
            return Ok(());
        }
    }
    flush(rect)
}

/// Copies `rect` of `from` (`0x00RRGGBB`) to `to`, packed in `to`'s format. The
/// loops walk raw pointers: unoptimised iterators cost a call a pixel.
fn pack_rows(from: Surface, to: Surface, rect: Rect) {
    let format = to.format;
    for y in rect.y as usize..(rect.y + rect.height) as usize {
        let mut src = from
            .base
            .as_ptr()
            .wrapping_add(y * from.stride + rect.x as usize);
        let mut dst = to
            .base
            .as_ptr()
            .wrapping_add(y * to.stride + rect.x as usize);
        let end = src.wrapping_add(rect.width as usize);
        while src < end {
            // SAFETY: `rect` is clamped to both surfaces, which are as large as
            // the screen and mapped; the framebuffer is device memory, written
            // volatile.
            unsafe { dst.write_volatile(format.pack(src.read())) };
            (src, dst) = (src.wrapping_add(1), dst.wrapping_add(1));
        }
    }
}

pub fn kind() -> Option<Kind> {
    DISPLAY
        .lock()
        .as_ref()
        .map(|display| match display.backend {
            Backend::Gop => Kind::Gop,
            Backend::Gpu(_) => Kind::VirtioGpu,
        })
}

/// Puts `rect` of the back buffer on screen. Never waits for the display: if
/// another CPU (or a fault on this one) holds it, the next flush sends the whole
/// screen instead, and this one returns an error.
pub fn flush(rect: Rect) -> Result<(), &'static str> {
    let Some(mut guard) = DISPLAY.try_lock() else {
        BEHIND.store(true, Ordering::Relaxed);
        return Err("display: busy");
    };
    let Some(display) = guard.as_mut() else {
        return Ok(());
    };
    let Backend::Gpu(gpu) = &mut display.backend else {
        return Ok(());
    };
    let rect = match BEHIND.swap(false, Ordering::Relaxed) {
        true => Rect::screen(display.surface.width as u32, display.surface.height as u32),
        false => rect,
    };
    gpu.flush(rect)
}

/// Runs `f` on the virtio-gpu, if the display is one.
fn with_gpu<R>(f: impl FnOnce(&mut Gpu) -> Result<R, &'static str>) -> Result<R, &'static str> {
    match DISPLAY.lock().as_mut().map(|display| &mut display.backend) {
        Some(Backend::Gpu(gpu)) => f(gpu),
        _ => Err("display: no hardware cursor without virtio-gpu"),
    }
}

/// Shows the hardware cursor: a 64x64 `0xAARRGGBB` image, its hot spot at `at`.
pub fn set_cursor(
    image: impl Fn(u32, u32) -> u32,
    hot: (u32, u32),
    at: (u32, u32),
) -> Result<(), &'static str> {
    with_gpu(|gpu| gpu.set_cursor(image, hot, at))
}

/// Moves the hardware cursor's hot spot to `at`.
pub fn move_cursor(at: (u32, u32)) -> Result<(), &'static str> {
    with_gpu(|gpu| gpu.move_cursor(at))
}
