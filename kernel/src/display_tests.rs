//! P3's display check (`test=display`): the console steps aside, the display
//! shows `gfxcore::pattern` over the whole screen, and the full-screen and one
//! text row flushes are timed. On virtio-gpu the hardware cursor is set and then
//! moved. Then the kernel waits for Enter while xtask takes a QEMU screendump and
//! compares it with the pattern pixel for pixel, and reads QEMU's cursor trace.

use crate::display::{self, Kind};
use crate::keyboard::Decoder;
use crate::{console, hpet, log};
use gfxcore::Rect;
use gfxcore::pattern::{CURSOR_AT, CURSOR_TO, cursor_pixel, pixel};
use termcore::keys::Key;

const FLUSHES: u64 = 20;
const TIMEOUT_NS: u64 = 30_000_000_000;

pub fn check() -> Result<(), &'static str> {
    let surface = display::surface().ok_or("display: no display")?;
    let kind = display::kind().ok_or("display: no display")?;
    let (width, height) = (surface.width as u32, surface.height as u32);
    log!(
        "display: {} {width}x{height}, stride {} pixels",
        match kind {
            Kind::Gop => "gop",
            Kind::VirtioGpu => "virtio-gpu",
        },
        surface.stride
    );
    console::detach();
    for y in 0..height {
        for x in 0..width {
            let at = y as usize * surface.stride + x as usize;
            // SAFETY: x and y are inside the surface, whose rows are `stride` long.
            unsafe {
                surface
                    .base
                    .as_ptr()
                    .add(at)
                    .write_volatile(pixel(x, y, width, height))
            };
        }
    }
    let full = time(Rect::screen(width, height))?;
    let row = time(Rect::new(0, height / 2, width, console::cell().1 as u32))?;
    log!("display: flush timing, mean of {FLUSHES}: full screen {full} us, one text row {row} us");
    if kind == Kind::VirtioGpu {
        display::set_cursor(cursor_pixel, (0, 0), CURSOR_AT)?;
        log!("display: cursor set at {} {}", CURSOR_AT.0, CURSOR_AT.1);
        display::move_cursor(CURSOR_TO)?;
        log!("display: cursor moved to {} {}", CURSOR_TO.0, CURSOR_TO.1);
    }
    log!("display: ready for keys (a screendump, then Enter)");
    let mut decoder = Decoder::new();
    let start = hpet::nanos();
    loop {
        if let Some(Key::Enter) = decoder.next_key() {
            return Ok(());
        }
        if hpet::nanos() - start > TIMEOUT_NS {
            return Err("display: no Enter within 30 s");
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

/// Mean microseconds per flush of `rect`.
fn time(rect: Rect) -> Result<u64, &'static str> {
    let start = hpet::nanos();
    for _ in 0..FLUSHES {
        display::flush(rect)?;
    }
    Ok((hpet::nanos() - start) / FLUSHES / 1000)
}
