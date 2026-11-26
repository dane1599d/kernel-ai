//! The fuzz target's body for `kaimedia`'s decoders (P6 groundwork G8, plan
//! M-1): PNG, JPEG, GIF and WebP from untrusted bytes. `fuzz_targets/
//! media_decode.rs` calls it with libFuzzer's input, and
//! `kaimedia/tests/hostile.rs` includes this file and runs it over hostile
//! mutations of its samples, which also seed the target's corpus. It must
//! return for any input; only a bug in the code under it may panic.

#![allow(dead_code)]

use kaimedia::{Limits, decode, probe};

/// Ceilings small enough that libFuzzer's 2 GiB never runs out.
pub const LIMITS: Limits = Limits {
    max_input: 1 << 20,
    max_side: 4_096,
    max_pixels: 4_000_000,
    max_frames: 64,
    max_bytes: 96 << 20,
};

/// The `media_decode` target: bytes probed and decoded. What decodes keeps
/// every ceiling, and each frame is the whole canvas as RGBA8.
pub fn media_decode(bytes: &[u8]) {
    if let Ok(info) = probe(bytes, &LIMITS) {
        assert!(info.width <= LIMITS.max_side && info.height <= LIMITS.max_side);
        assert!(info.frames.is_none_or(|n| n <= LIMITS.max_frames));
    }
    if let Ok(image) = decode(bytes, &LIMITS) {
        assert!(!image.frames.is_empty());
        assert!(image.frames.len() as u32 <= LIMITS.max_frames);
        assert!(u64::from(image.width) * u64::from(image.height) <= LIMITS.max_pixels);
        let canvas = image.width as usize * image.height as usize * 4;
        let mut total = 0u64;
        for frame in &image.frames {
            assert_eq!(frame.rgba.len(), canvas);
            total += canvas as u64;
        }
        assert!(total <= LIMITS.max_bytes);
    }
}
