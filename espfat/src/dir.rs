//! Directory entries: 8.3 names, and the long-name entries before them.

use crate::volume::Volume;
use crate::{Disk, Error, SECTOR, u16_at, u32_at};

pub(crate) const ATTR_DIR: u8 = 0x10;
const ATTR_VOLUME: u8 = 0x08;
const ATTR_LFN: u8 = 0x0F;
/// UTF-16 units one long-name entry holds.
const LFN_UNITS: usize = 13;
/// The most long-name entries one name takes: 255 units.
const LFN_PARTS: usize = 20;
/// Where a long-name entry's 13 units sit.
const LFN_AT: [usize; LFN_UNITS] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

/// A directory entry found by name, and where its 8.3 entry lies.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Found {
    pub attr: u8,
    pub first: u32,
    pub size: u32,
    /// The disk sector holding the 8.3 entry, and the entry's offset in it.
    pub lba: u64,
    pub at: usize,
}

/// A long name being gathered, last part first, as the entries come.
struct Long {
    units: [u16; LFN_UNITS * LFN_PARTS],
    /// The part expected next. Zero once every part has come.
    expect: u8,
    sum: u8,
    active: bool,
}

impl Long {
    fn new() -> Self {
        Self {
            units: [0xFFFF; LFN_UNITS * LFN_PARTS],
            expect: 0,
            sum: 0,
            active: false,
        }
    }

    fn take(&mut self, e: &[u8]) {
        let part = e[0] & 0x1F;
        if e[0] & 0x40 != 0 {
            self.units.fill(0xFFFF);
            self.active = (1..=LFN_PARTS as u8).contains(&part);
            self.expect = part;
            self.sum = e[13];
        }
        if !self.active || part != self.expect || part == 0 || e[13] != self.sum {
            self.active = false;
            return;
        }
        let base = usize::from(part - 1) * LFN_UNITS;
        for (i, &at) in LFN_AT.iter().enumerate() {
            self.units[base + i] = u16_at(e, at);
        }
        self.expect -= 1;
    }

    /// The name, if every part came and they belong to `short`.
    fn name(&self, short: &[u8]) -> Option<&[u16]> {
        if !self.active || self.expect != 0 || checksum(short) != self.sum {
            return None;
        }
        let end = self
            .units
            .iter()
            .position(|&u| u == 0 || u == 0xFFFF)
            .unwrap_or(self.units.len());
        Some(&self.units[..end])
    }
}

fn checksum(short: &[u8]) -> u8 {
    short[..11]
        .iter()
        .fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b))
}

fn fold(unit: u16) -> u16 {
    match unit {
        0x61..=0x7A => unit - 0x20,
        _ => unit,
    }
}

fn long_matches(long: &[u16], name: &str) -> bool {
    let mut units = name.encode_utf16();
    long.iter()
        .all(|&u| units.next().map(fold) == Some(fold(u)))
        && units.next().is_none()
}

/// Whether the 8.3 name in `e` is `name`, without regard to ASCII case.
fn short_matches(e: &[u8], name: &str) -> bool {
    let mut base = [0u8; 8];
    base.copy_from_slice(&e[..8]);
    if base[0] == 0x05 {
        base[0] = 0xE5;
    }
    let trim = |b: &[u8]| b.len() - b.iter().rev().take_while(|&&c| c == b' ').count();
    let base = &base[..trim(&base)];
    let ext = &e[8..8 + trim(&e[8..11])];
    let name = name.as_bytes();
    let (want_base, want_ext) = match ext.is_empty() {
        true => (name, &[][..]),
        false => match name.iter().rposition(|&c| c == b'.') {
            Some(dot) => (&name[..dot], &name[dot + 1..]),
            None => return false,
        },
    };
    want_base.eq_ignore_ascii_case(base) && want_ext.eq_ignore_ascii_case(ext)
}

impl Found {
    fn at(e: &[u8], lba: u64, at: usize) -> Self {
        Self {
            attr: e[11],
            first: (u32::from(u16_at(e, 20)) << 16) | u32::from(u16_at(e, 26)),
            size: u32_at(e, 28),
            lba,
            at,
        }
    }

    /// Whether this is a directory's "." or ".." entry.
    fn is_dot(e: &[u8]) -> bool {
        &e[..11] == b".          " || &e[..11] == b"..         "
    }
}

impl<D: Disk> Volume<D> {
    /// Calls `each` with every file and directory entry in the directory that
    /// starts at `first`, read through FAT copy `copy`, with the long name that
    /// belongs to it, until `each` returns something. "." and ".." are never
    /// given to `each`, whatever long name comes before them, so a lookup and
    /// the whole-volume walk skip the same entries.
    pub(crate) fn scan<T>(
        &mut self,
        copy: u32,
        first: u32,
        mut each: impl FnMut(&[u8], Option<&[u16]>, Found) -> Option<T>,
    ) -> Result<Option<T>, Error> {
        let g = self.geometry;
        let mut long = Long::new();
        for cluster in self.dir_chain(copy, first)? {
            for n in 0..g.sectors_per_cluster {
                let lba = g.cluster_lba(cluster, n);
                let sector = *self.sector(lba)?;
                for at in (0..SECTOR).step_by(32) {
                    let e = &sector[at..at + 32];
                    match (e[0], e[11]) {
                        (0x00, _) => return Ok(None),
                        (0xE5, _) => long.active = false,
                        (_, attr) if attr & 0x3F == ATTR_LFN => long.take(e),
                        (_, attr) if attr & ATTR_VOLUME != 0 => long.active = false,
                        _ if Found::is_dot(e) => long.active = false,
                        _ => {
                            let got = each(e, long.name(e), Found::at(e, lba, at));
                            long.active = false;
                            if got.is_some() {
                                return Ok(got);
                            }
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    /// The entry named `name` in the directory that starts at `first`.
    pub(crate) fn lookup(&mut self, first: u32, name: &str) -> Result<Found, Error> {
        let hit = |e: &[u8], long: Option<&[u16]>, found: Found| {
            let by_long = long.is_some_and(|units| long_matches(units, name));
            (by_long || short_matches(e, name)).then_some(found)
        };
        self.scan(self.read_fat, first, hit)?.ok_or(Error::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(name: &[u8; 11]) -> [u8; 32] {
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(name);
        e
    }

    #[test]
    fn short_names_match_without_case() {
        assert!(short_matches(&short(b"BOOTX64 EFI"), "bootx64.efi"));
        assert!(short_matches(&short(b"LIMINE     "), "Limine"));
        assert!(!short_matches(&short(b"LIMINE     "), "limine."));
        assert!(!short_matches(&short(b"BOOTX64 EFI"), "bootx64"));
        assert!(!short_matches(&short(b"BOOTX64 EFI"), "bootx64.ef"));
    }

    #[test]
    fn a_long_name_needs_every_part_and_the_checksum() {
        let s = short(b"KERNEL~1LIN");
        let units: alloc::vec::Vec<u16> = "kernel.lines".encode_utf16().collect();
        let mut e = [0u8; 32];
        e[0] = 0x41;
        e[11] = ATTR_LFN;
        e[13] = checksum(&s);
        for (i, &at) in LFN_AT.iter().enumerate() {
            let u = units
                .get(i)
                .copied()
                .unwrap_or(if i == units.len() { 0 } else { 0xFFFF });
            e[at..at + 2].copy_from_slice(&u.to_le_bytes());
        }
        let mut long = Long::new();
        long.take(&e);
        assert!(long_matches(long.name(&s).unwrap(), "Kernel.Lines"));
        assert!(long.name(&short(b"KERNEL~2LIN")).is_none());
        // A part out of order drops the name.
        let mut long = Long::new();
        e[0] = 0x42;
        long.take(&e);
        assert!(long.name(&s).is_none());
    }
}
