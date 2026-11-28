//! The `dev_cfg` fuzz target's body (ADR 0007 section 5 and 12, P8 row 1),
//! which `pci/tests/hostile.rs` also runs over hostile spaces:
//! `fuzz/fuzz_targets/dev_cfg.rs` includes this file by `#[path]` and calls
//! [`dev_cfg`] with libFuzzer's input. It must return for any input, and only
//! a bug in `pci::cfg` may panic.
//!
//! The input is `[flags, width, offset (u16 LE), value (u32 LE)]` and then the
//! function's configuration space as the function reads (cut or padded with
//! zeros to 256 bytes, or to 4096 when flags bit 0 is set). The promise: a
//! write [`allow_write`] allows never changes a header byte but the command
//! register's Memory Space and Bus Master bits, never a byte of an MSI or
//! MSI-X capability, a capability's ID or next byte, or the body of SR-IOV or
//! a Resizable BAR; and a list the reference walk below finds malformed
//! refuses every write it should. The reference walk is written apart from the
//! filter's (a hop count, not a seen set), so a bug in one shows against the
//! other.

#![allow(dead_code)]

use pci::cfg::{Refused, allow_write};

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

/// Runs the filter and holds it to the promise.
pub fn check(space: &[u8], offset: u16, width: u8, value: u32) {
    let verdict = allow_write(space, offset, width, value);
    let (start, w) = (usize::from(offset), usize::from(width));
    let sane = matches!(width, 1 | 2 | 4) && start % w.max(1) == 0 && start + w <= space.len();
    if !sane {
        assert!(verdict.is_err(), "an ill-formed write was allowed");
        return;
    }
    let end = start + w;
    let standard = reference_standard(space);
    let extended = reference_extended(space);
    // A list that is malformed refuses what the filter's module doc says.
    if standard.is_none() && start >= 0x40 {
        assert_eq!(verdict, Err(Refused::BadList), "{offset:#x}");
    }
    if standard.is_some() && extended.is_none() && start >= 0x100 {
        assert_eq!(verdict, Err(Refused::BadExtended), "{offset:#x}");
    }
    if verdict.is_err() {
        return;
    }
    // Allowed: apply it, and see what changed.
    let mut after = space.to_vec();
    for (i, byte) in after[start..end].iter_mut().enumerate() {
        *byte = (value >> (8 * i)) as u8;
    }
    let changed = |at: usize| after[at] != space[at];
    for at in start..end {
        if !changed(at) {
            continue;
        }
        match at {
            4 | 5 => {
                let old = u16::from_le_bytes([space[4], space[5]]);
                let new = u16::from_le_bytes([after[4], after[5]]);
                assert_eq!((old ^ new) & !0b110, 0, "command bits {old:#x} to {new:#x}");
            }
            0..0x40 => panic!("header byte {at:#x} changed"),
            _ => {
                let protected = standard
                    .iter()
                    .flatten()
                    .chain(extended.iter().flatten())
                    .any(|&(a, b)| a <= at && at < b);
                assert!(!protected, "protected byte {at:#x} changed");
            }
        }
    }
}

/// The byte ranges the standard list protects, by a plain hop-counted walk, or
/// None when it is malformed. No list (Status bit 4 clear) protects nothing.
fn reference_standard(space: &[u8]) -> Option<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    if space[6] & 0x10 == 0 {
        return Some(out);
    }
    let mut at = usize::from(space[0x34] & 0xFC);
    for _hop in 0..=48 {
        if at == 0 {
            return Some(out);
        }
        if !(0x40..=0xFC).contains(&at) {
            return None;
        }
        out.push((at, at + 2));
        let body = match space[at] {
            0x05 => {
                let control = u16::from_le_bytes([space[at + 2], space[at + 3]]);
                match (control & 0x80 != 0, control & 0x100 != 0) {
                    (false, false) => 10,
                    (true, false) => 14,
                    (false, true) => 20,
                    (true, true) => 24,
                }
            }
            0x11 => 12,
            _ => 0,
        };
        if body > 0 {
            out.push((at, (at + body).min(0x100)));
        }
        at = usize::from(space[at + 1] & 0xFC);
    }
    None
}

/// The byte ranges the extended list protects, or None when it is malformed
/// (more than 960 hops cannot be a list that visits each dword once).
fn reference_extended(space: &[u8]) -> Option<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    if space.len() < 4096 {
        return Some(out);
    }
    let dword = |at: usize| u32::from_le_bytes(space[at..at + 4].try_into().unwrap());
    if dword(0x100) == 0 {
        return Some(out);
    }
    let mut caps = Vec::new();
    let mut at = 0x100;
    for _hop in 0..=960 {
        let header = dword(at);
        if header == u32::MAX {
            return None;
        }
        caps.push((at, (header & 0xFFFF) as u16));
        out.push((at, at + 4));
        let next = ((header >> 20) & 0xFFC) as usize;
        match next {
            0 => {
                for &(start, id) in &caps {
                    if matches!(id, 0x10 | 0x15 | 0x24) {
                        let end = caps.iter().map(|c| c.0).filter(|&s| s > start).min();
                        out.push((start + 4, end.unwrap_or(4096)));
                    }
                }
                return Some(out);
            }
            n if n < 0x100 => return None,
            n => at = n,
        }
    }
    None
}
