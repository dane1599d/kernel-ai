//! Minimal framebuffer output for M0: fill the screen with one colour.

use limine::framebuffer::Framebuffer;

/// Background colour (0x00RRGGBB in a 32-bit linear framebuffer).
const BACKGROUND: u32 = 0x0010_2A43;

/// Fills a 32-bpp framebuffer. Returns false for formats M0 does not handle.
pub fn fill(fb: &Framebuffer) -> bool {
    if fb.bpp != 32 {
        return false;
    }
    let base = fb.address().cast::<u32>();
    let pixels_per_row = (fb.pitch / 4) as usize;
    for y in 0..fb.height as usize {
        for x in 0..fb.width as usize {
            // SAFETY: (x, y) is inside the framebuffer Limine mapped for us.
            unsafe { base.add(y * pixels_per_row + x).write_volatile(BACKGROUND) };
        }
    }
    true
}
