//! The `display` cases' host checks: QEMU's screendump (a binary PPM) must equal
//! `gfxcore::pattern` pixel for pixel, at the size the case expects, and on
//! virtio-gpu QEMU's cursor trace must show the cursor set and then moved.

use gfxcore::pattern::{CURSOR_AT, CURSOR_TO, pixel};
use std::path::Path;

/// The screendump the case's keys ask QEMU for, in `target/`.
pub const DUMP: &str = "display.ppm";
/// QEMU's log, with the `virtio_gpu_update_cursor` trace event on, in `target/`.
pub const TRACE: &str = "display-trace.log";
/// The trace event that records each cursor request virtio-gpu handles.
pub const TRACE_EVENT: &str = "virtio_gpu_update_cursor";

/// What a case's screendump must be.
#[derive(Clone, Copy)]
pub struct Expect {
    /// Width and height; None takes the dump's own (the size the kernel chose).
    pub size: Option<(u32, u32)>,
    /// The cursor trace must show the set and the move.
    pub cursor: bool,
}

/// The QEMU monitor commands, then Enter, that the case types once the kernel
/// shows the pattern.
pub const KEYS: &[&str] = &["!screendump display.ppm", "ret"];

/// Removes what an earlier run left, so a stale dump cannot pass.
pub fn clear(target_dir: &Path) {
    let _ = std::fs::remove_file(target_dir.join(DUMP));
    let _ = std::fs::remove_file(target_dir.join(TRACE));
}

/// QEMU arguments for a case on virtio-gpu: no VGA beside it, and the cursor
/// trace in TRACE.
pub fn gpu_args() -> Vec<String> {
    [
        "-vga",
        "none",
        "-device",
        "virtio-gpu-pci",
        "-D",
        TRACE,
        "-trace",
        TRACE_EVENT,
    ]
    .map(String::from)
    .to_vec()
}

pub fn check(target_dir: &Path, expect: Expect) -> Result<(), String> {
    let path = target_dir.join(DUMP);
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("FAIL: no screendump at {}: {e}", path.display()))?;
    let (width, height, pixels) = parse_ppm(&bytes)?;
    if let Some(size) = expect.size
        && size != (width, height)
    {
        return Err(format!(
            "FAIL: the screen is {width}x{height}, not {}x{}",
            size.0, size.1
        ));
    }
    compare(width, height, pixels)?;
    println!("xtask: the {width}x{height} screendump matches the test pattern pixel for pixel");
    if expect.cursor {
        let trace = std::fs::read_to_string(target_dir.join(TRACE))
            .map_err(|e| format!("FAIL: no QEMU trace log {TRACE}: {e}"))?;
        check_cursor(&trace)?;
        println!("xtask: QEMU's trace shows the cursor set and then moved");
    }
    Ok(())
}

/// Width, height and RGB bytes of a binary PPM with 8-bit channels.
fn parse_ppm(bytes: &[u8]) -> Result<(u32, u32, &[u8]), String> {
    let mut fields = Vec::new();
    let mut at = 0;
    while fields.len() < 4 {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if bytes.get(at) == Some(&b'#') {
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        let start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if start == at {
            return Err("FAIL: the screendump's PPM header is cut short".into());
        }
        fields.push(String::from_utf8_lossy(&bytes[start..at]).into_owned());
    }
    // One whitespace byte separates the header from the pixels.
    let pixels = &bytes[(at + 1).min(bytes.len())..];
    let number = |field: &str| {
        field
            .parse::<u32>()
            .map_err(|_| format!("FAIL: bad PPM header field {field:?}"))
    };
    if fields[0] != "P6" || fields[3] != "255" {
        return Err(format!("FAIL: not an 8-bit binary PPM: {fields:?}"));
    }
    let (width, height) = (number(&fields[1])?, number(&fields[2])?);
    if pixels.len() != width as usize * height as usize * 3 {
        return Err(format!(
            "FAIL: the PPM holds {} bytes of pixels, not {width}x{height}x3",
            pixels.len()
        ));
    }
    Ok((width, height, pixels))
}

fn compare(width: u32, height: u32, pixels: &[u8]) -> Result<(), String> {
    let mut wrong = 0u64;
    let mut first = None;
    for (index, rgb) in pixels.as_chunks::<3>().0.iter().enumerate() {
        let (x, y) = (index as u32 % width, index as u32 / width);
        let got = u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
        let want = pixel(x, y, width, height);
        if got != want {
            wrong += 1;
            first.get_or_insert((x, y, got, want));
        }
    }
    match first {
        None => Ok(()),
        Some((x, y, got, want)) => Err(format!(
            "FAIL: {wrong} pixels differ from the test pattern; the first at {x},{y} is {got:06x}, not {want:06x}"
        )),
    }
}

/// The trace must show an update at CURSOR_AT and, after it, a move to CURSOR_TO.
fn check_cursor(trace: &str) -> Result<(), String> {
    let lines: Vec<&str> = trace.lines().filter(|l| l.contains(TRACE_EVENT)).collect();
    let set = lines
        .iter()
        .position(|l| is_cursor(l, CURSOR_AT, "update"))
        .ok_or_else(|| {
            format!("FAIL: the trace never shows the cursor set at {CURSOR_AT:?}: {lines:?}")
        })?;
    if !lines[set..].iter().any(|l| is_cursor(l, CURSOR_TO, "move")) {
        return Err(format!(
            "FAIL: the trace never shows the cursor moved to {CURSOR_TO:?}: {lines:?}"
        ));
    }
    Ok(())
}

/// Whether a `virtio_gpu_update_cursor` line is a `kind` request on scanout 0 at
/// `at`. QEMU's wording changes between versions (`scanout 0 @ 100,100 type
/// update` and `scanout 0, x 100, y 100, update`), so only its words and its
/// first three numbers count.
fn is_cursor(line: &str, at: (u32, u32), kind: &str) -> bool {
    let event = line
        .find(TRACE_EVENT)
        .map_or(line, |i| &line[i + TRACE_EVENT.len()..]);
    let words: Vec<&str> = event
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let numbers: Vec<u32> = words
        .iter()
        .filter_map(|w| w.parse().ok())
        .take(3)
        .collect();
    words.contains(&kind) && numbers == [0, at.0, at.1]
}

#[cfg(test)]
mod tests {
    use super::{check_cursor, compare, parse_ppm};
    use gfxcore::pattern::pixel;

    fn ppm(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = format!("P6\n# QEMU\n{width} {height}\n255\n").into_bytes();
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&pixel(x, y, width, height).to_be_bytes()[1..]);
            }
        }
        bytes
    }

    #[test]
    fn a_dump_of_the_pattern_passes_and_one_wrong_pixel_fails() {
        let mut bytes = ppm(64, 48);
        let (w, h, pixels) = parse_ppm(&bytes).unwrap();
        assert_eq!((w, h), (64, 48));
        assert!(compare(w, h, pixels).is_ok());
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        let (w, h, pixels) = parse_ppm(&bytes).unwrap();
        let error = compare(w, h, pixels).unwrap_err();
        assert!(
            error.contains("1 pixels differ") && error.contains("63,47"),
            "{error}"
        );
    }

    #[test]
    fn a_short_or_foreign_ppm_is_refused() {
        let bytes = ppm(8, 8);
        assert!(parse_ppm(&bytes[..bytes.len() - 1]).is_err());
        assert!(parse_ppm(b"P5\n8 8\n255\n").is_err());
        assert!(parse_ppm(b"P6\n8").is_err());
    }

    #[test]
    fn the_cursor_must_be_set_then_moved() {
        let set = "virtio_gpu_update_cursor scanout 0 @ 100,100 type update res 2";
        let moved = "virtio_gpu_update_cursor scanout 0 @ 300,200 type move res 0";
        assert!(check_cursor(&format!("{set}\n{moved}\n")).is_ok());
        assert!(check_cursor(&format!("{moved}\n{set}\n")).is_err());
        assert!(check_cursor(set).is_err());
    }
}
