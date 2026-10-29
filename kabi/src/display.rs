//! The screen as `desktop` owns it (ADR 0005 §2): DISPLAY_CLAIM's answer and
//! DISPLAY_FLUSH's rectangles.

/// What DISPLAY_CLAIM writes: the back buffer's shape. Rows are `stride` bytes
/// apart, pixels XRGB8888 (`0x00RRGGBB` little-endian u32), row 0 at the top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
    /// [`FORMAT_XRGB8888`].
    pub format: u32,
    /// The screen's size in mm from its EDID, 0 when unknown.
    pub width_mm: u32,
    pub height_mm: u32,
    /// Pages in the `Shm`: `stride * height` rounded up to 4 KiB.
    pub pages: u32,
    /// 1 with a hardware cursor (virtio-gpu), 0 without (GOP).
    pub cursor: u32,
}

/// Wayland's `wl_shm` code for XRGB8888.
pub const FORMAT_XRGB8888: u32 = 1;

/// The most rectangles one DISPLAY_FLUSH takes.
pub const MAX_FLUSH: usize = 32;

/// A rectangle of the back buffer, in pixels; cut to the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct FlushRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Side of the square hardware cursor image, in pixels.
pub const CURSOR: usize = 64;
