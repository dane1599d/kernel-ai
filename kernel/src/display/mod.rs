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
}

// SAFETY: the device state is used only under the DISPLAY mutex, by one CPU at a
// time; nothing in it is tied to a particular CPU.
unsafe impl Send for Display {}

static DISPLAY: Mutex<Option<Display>> = Mutex::new(None);
/// A flush found the display busy: the next one sends the whole screen.
static BEHIND: AtomicBool = AtomicBool::new(false);

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
    };
    *DISPLAY.lock() = Some(Display {
        surface,
        backend: Backend::Gop,
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
    };
    gpu.flush(Rect::screen(mode.0, mode.1))?;
    *DISPLAY.lock() = Some(Display {
        surface,
        backend: Backend::Gpu(Box::new(gpu)),
    });
    Ok(surface)
}

/// The display's back buffer, if there is a display.
pub fn surface() -> Option<Surface> {
    DISPLAY.lock().as_ref().map(|display| display.surface)
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
