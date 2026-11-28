//! The `dev_cfg` fuzz target's body (ADR 0007 section 5 and 12, P8 row 1),
//! which `pci/tests/hostile.rs` also runs over hostile spaces:
//! `fuzz/fuzz_targets/dev_cfg.rs` includes this file by `#[path]` and calls
//! [`dev_cfg`] with libFuzzer's input. It must return for any input, and only
//! a bug in `pci::cfg` may panic.
//!
//! The input is `[flags, width, offset (u16 LE), value (u32 LE)]` and then the
//! function's configuration space as the function reads (cut or padded with
//! zeros to 256 bytes, or to 4096 when flags bit 0 is set). The promise is
//! held against [`Model`], which is made from the specification's lengths and
//! is written apart from the filter (no clipping to the space, a hop count
//! where the filter keeps a seen set, one pass): a write [`allow_write`] allows
//! never overlaps a byte the model says a capability owns (not just changes
//! one, so a write of the same value to Status or an MSI-X control word is
//! caught), and what the model finds malformed is refused. Writes to the
//! header are checked exactly: allowed if and only if they are the command
//! register changing Memory Space and Bus Master alone.

#![allow(dead_code)]

use pci::cfg::allow_write;

pub const HEADER_BYTES: usize = 8;

/// An input for a write of `width` bytes of `value` at `offset` in `space`.
pub fn input(space: &[u8], width: u8, offset: u16, value: u32) -> Vec<u8> {
    let mut out = vec![u8::from(space.len() == 4096), width];
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&value.to_le_bytes());
    out.extend_from_slice(space);
    out
}

pub fn dev_cfg(data: &[u8]) {
    let Some(head) = data.get(..HEADER_BYTES) else {
        return;
    };
    let len = if head[0] & 1 != 0 { 4096 } else { 256 };
    let mut space = data[HEADER_BYTES..].to_vec();
    space.resize(len, 0);
    let width = head[1];
    let offset = u16::from_le_bytes([head[2], head[3]]);
    let value = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
    check(&space, offset, width, value);
}

/// Runs the filter and holds it to the promise, by a model of the device made
/// from the specification's own lengths (never clipped to the space, and
/// without the filter's walk, its seen sets or its two passes).
pub fn check(space: &[u8], offset: u16, width: u8, value: u32) {
    let verdict = allow_write(space, offset, width, value);
    let (start, w) = (usize::from(offset), usize::from(width));
    let sane = matches!(width, 1 | 2 | 4) && start % w.max(1) == 0 && start + w <= space.len();
    let sane = sane && (w == 4 || value >> (8 * w) == 0);
    if !sane {
        assert!(verdict.is_err(), "an ill-formed write was allowed");
        return;
    }
    let end = start + w;
    // The header: exactly the command register's two allowed bits, and
    // nothing else, whatever the lists hold.
    if start < 0x40 {
        let old = u16::from_le_bytes([space[4], space[5]]);
        let mut new = old.to_le_bytes();
        for at in start.max(4)..end.min(6) {
            new[at - 4] = (value >> (8 * (at - start))) as u8;
        }
        let in_command = start >= 4 && end <= 6;
        let wanted = in_command && (old ^ u16::from_le_bytes(new)) & !0b110 == 0;
        assert_eq!(verdict.is_ok(), wanted, "header write {offset:#x}/{width}");
        return;
    }
    let model = Model::of(space);
    if model.refuses(start, end) {
        assert!(
            verdict.is_err(),
            "{offset:#x}/{width} touches {:?}",
            model.why(start, end)
        );
    }
    if verdict.is_err() {
        return;
    }
    // Allowed: apply it, and see that no protected byte changed.
    let mut after = space.to_vec();
    for (i, byte) in after[start..end].iter_mut().enumerate() {
        *byte = (value >> (8 * i)) as u8;
    }
    for at in start..end {
        assert!(
            after[at] == space[at] || !model.refuses(at, at + 1),
            "protected byte {at:#x} changed"
        );
    }
}

/// What a device's space says about the bytes it owns, from the spec.
struct Model {
    /// Every write from 0x40 up (or from 0x100 up) is refused.
    all_above_header: bool,
    all_extended: bool,
    /// Byte ranges the capabilities own.
    ranges: Vec<(usize, usize)>,
}

impl Model {
    fn of(space: &[u8]) -> Model {
        let mut model = Model {
            all_above_header: space[0x0E] & 0x7F > 1,
            all_extended: false,
            ranges: Vec::new(),
        };
        if space[6] & 0x10 != 0 && !model.all_above_header {
            model.standard(space);
        }
        if !model.all_above_header {
            model.extended(space);
        }
        model
    }

    fn standard(&mut self, space: &[u8]) {
        let mut at = usize::from(space[0x34] & 0xFC);
        let mut visited = Vec::new();
        while at != 0 {
            // A pointer out of range, a revisit, or more hops than a dword
            // each in 0x40..0x100 allows.
            if !(0x40..=0xFC).contains(&at) || visited.contains(&at) || visited.len() == 48 {
                self.all_above_header = true;
                return;
            }
            visited.push(at);
            self.ranges.push((at, at + 2));
            let control = u16::from_le_bytes([space[at + 2], space[at + 3]]);
            let len = match space[at] {
                // MSI: [32-bit, 64-bit][no mask, per-vector mask] bytes, with
                // Extended Message Data counted in the first two.
                0x05 => [[12, 20], [16, 24]][usize::from(control >> 7 & 1)]
                    [usize::from(control >> 8 & 1)],
                0x11 => 12,
                0x14 | 0x08 => {
                    self.all_above_header = true;
                    return;
                }
                _ => 0,
            };
            if at + len > 0x100 {
                self.all_above_header = true;
                return;
            }
            self.ranges.push((at, at + len));
            at = usize::from(space[at + 1] & 0xFC);
        }
    }

    fn extended(&mut self, space: &[u8]) {
        if space.len() < 4096 || space[0x100..0x104] == [0; 4] {
            return;
        }
        let dword = |at: usize| u32::from_le_bytes(space[at..at + 4].try_into().unwrap());
        let mut caps: Vec<(usize, u16)> = Vec::new();
        let mut at = 0x100;
        loop {
            let header = dword(at);
            if header == u32::MAX || caps.iter().any(|c| c.0 == at) || caps.len() == 960 {
                self.all_extended = true;
                return;
            }
            caps.push((at, header as u16));
            match (header >> 20 & 0xFFC) as usize {
                0 => break,
                n if n < 0x100 => {
                    self.all_extended = true;
                    return;
                }
                n => at = n,
            }
        }
        for &(start, id) in &caps {
            self.ranges.push((start, start + 4));
            let len = match id {
                0x0010 => 0x40,
                0x0015 | 0x0024 => 0x34,
                0x0012 => {
                    self.all_extended = true;
                    return;
                }
                _ => continue,
            };
            if caps.iter().any(|c| c.0 > start && c.0 < start + len) {
                self.all_extended = true;
                return;
            }
            self.ranges.push((start, (start + len).min(4096)));
        }
    }

    fn refuses(&self, start: usize, end: usize) -> bool {
        (self.all_above_header && end > 0x40)
            || (self.all_extended && end > 0x100)
            || self.ranges.iter().any(|&(a, b)| a < end && start < b)
    }

    fn why(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        self.ranges
            .iter()
            .copied()
            .find(|&(a, b)| a < end && start < b)
    }
}
