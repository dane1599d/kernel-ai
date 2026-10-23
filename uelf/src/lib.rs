//! Reads and validates the static ELF64 executables the kernel runs in ring 3.
//! Kept free of kernel code so every rejection can be tested on the host.
//!
//! Only what the loader needs is accepted: a little-endian x86-64 `ET_EXEC` with up
//! to [`MAX_SEGMENTS`] `PT_LOAD` segments inside the allowed range, none both
//! writable and executable, none overlapping (after rounding to pages), and the
//! entry point inside an executable segment. Interpreters, dynamic linking and TLS
//! are refused. A `PT_NOTE` named "KAI" carries the ABI version the program was
//! built for ([`Image::abi`]); the loader decides what to accept.

#![no_std]

use core::ops::Range;

pub const MAX_SEGMENTS: usize = 8;
const PAGE: u64 = 4096;
const HEADER_SIZE: usize = 64;
const PH_SIZE: usize = 56;
const MAX_PROGRAM_HEADERS: usize = 16;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_TLS: u32 = 7;
/// The note that carries the ABI version: name "KAI", type 1, two u32s.
const KAI_NOTE_NAME: &[u8] = b"KAI\0";
const KAI_NOTE_TYPE: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    TooShort,
    NotElf64LittleEndian,
    NotX86_64Executable,
    BadProgramHeaders,
    TooManySegments,
    /// Interpreter, dynamic section or TLS.
    Unsupported,
    SegmentOutsideFile,
    SegmentOutOfRange,
    WritableAndExecutable,
    Overlap,
    NoSegments,
    BadEntry,
    /// A note runs past its segment or the file.
    BadNote,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Segment {
    pub vaddr: u64,
    pub memsz: u64,
    /// Bytes of the file copied to `vaddr`; the rest of `memsz` is zero.
    pub file: Range<usize>,
    pub write: bool,
    pub execute: bool,
}

impl Segment {
    /// The pages the segment covers.
    pub fn pages(&self) -> Range<u64> {
        let start = self.vaddr / PAGE * PAGE;
        let end = (self.vaddr + self.memsz).div_ceil(PAGE) * PAGE;
        start..end
    }
}

#[derive(Debug)]
pub struct Image {
    pub entry: u64,
    /// The (major, minor) ABI version in the "KAI" note; `None` without one.
    pub abi: Option<(u32, u32)>,
    segments: [Segment; MAX_SEGMENTS],
    count: usize,
}

impl Image {
    pub fn segments(&self) -> &[Segment] {
        &self.segments[..self.count]
    }

    /// Pages all segments cover together.
    pub fn page_count(&self) -> u64 {
        self.segments()
            .iter()
            .map(|s| (s.pages().end - s.pages().start) / PAGE)
            .sum()
    }
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap_or([0; 4]))
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap_or([0; 8]))
}

/// Parses `data`; every segment must lie within `allowed` (whole pages).
pub fn parse(data: &[u8], allowed: Range<u64>) -> Result<Image, Error> {
    if data.len() < HEADER_SIZE {
        return Err(Error::TooShort);
    }
    // Magic, ELFCLASS64, ELFDATA2LSB, EV_CURRENT.
    if data[..4] != [0x7F, b'E', b'L', b'F'] || data[4] != 2 || data[5] != 1 || data[6] != 1 {
        return Err(Error::NotElf64LittleEndian);
    }
    // ET_EXEC, EM_X86_64.
    if u16_at(data, 16) != 2 || u16_at(data, 18) != 62 {
        return Err(Error::NotX86_64Executable);
    }
    let entry = u64_at(data, 24);
    let ph_offset = usize::try_from(u64_at(data, 32)).map_err(|_| Error::BadProgramHeaders)?;
    let ph_size = usize::from(u16_at(data, 54));
    let ph_count = usize::from(u16_at(data, 56));
    if ph_size != PH_SIZE || ph_count > MAX_PROGRAM_HEADERS {
        return Err(Error::BadProgramHeaders);
    }
    let table_end = ph_count
        .checked_mul(PH_SIZE)
        .and_then(|size| ph_offset.checked_add(size))
        .ok_or(Error::BadProgramHeaders)?;
    if table_end > data.len() {
        return Err(Error::BadProgramHeaders);
    }
    let mut image = Image {
        entry,
        abi: None,
        segments: Default::default(),
        count: 0,
    };
    for index in 0..ph_count {
        let at = ph_offset + index * PH_SIZE;
        match u32_at(data, at) {
            PT_LOAD => {}
            PT_NOTE => {
                let offset = u64_at(data, at + 8);
                let filesz = u64_at(data, at + 32);
                let end = offset
                    .checked_add(filesz)
                    .filter(|&end| end <= data.len() as u64)
                    .ok_or(Error::BadNote)?;
                if let Some(abi) = kai_note(&data[offset as usize..end as usize])? {
                    image.abi = Some(abi);
                }
                continue;
            }
            PT_DYNAMIC | PT_INTERP | PT_TLS => return Err(Error::Unsupported),
            _ => continue,
        }
        if image.count == MAX_SEGMENTS {
            return Err(Error::TooManySegments);
        }
        let flags = u32_at(data, at + 4);
        let offset = u64_at(data, at + 8);
        let vaddr = u64_at(data, at + 16);
        let filesz = u64_at(data, at + 32);
        let memsz = u64_at(data, at + 40);
        let file_end = offset
            .checked_add(filesz)
            .filter(|&end| filesz <= memsz && end <= data.len() as u64)
            .ok_or(Error::SegmentOutsideFile)?;
        let end = vaddr
            .checked_add(memsz)
            .and_then(|end| end.checked_add(PAGE - 1))
            .ok_or(Error::SegmentOutOfRange)?;
        if memsz == 0 || vaddr / PAGE * PAGE < allowed.start || end / PAGE * PAGE > allowed.end {
            return Err(Error::SegmentOutOfRange);
        }
        let (write, execute) = (flags & PF_W != 0, flags & PF_X != 0);
        if write && execute {
            return Err(Error::WritableAndExecutable);
        }
        image.segments[image.count] = Segment {
            vaddr,
            memsz,
            file: offset as usize..file_end as usize,
            write,
            execute,
        };
        image.count += 1;
    }
    if image.count == 0 {
        return Err(Error::NoSegments);
    }
    let segments = image.segments();
    for (i, a) in segments.iter().enumerate() {
        for b in &segments[i + 1..] {
            if a.pages().start < b.pages().end && b.pages().start < a.pages().end {
                return Err(Error::Overlap);
            }
        }
    }
    let entry_ok = segments
        .iter()
        .any(|s| s.execute && (s.vaddr..s.vaddr + s.memsz).contains(&entry));
    if !entry_ok {
        return Err(Error::BadEntry);
    }
    Ok(image)
}

/// The ABI version in the "KAI" note among the notes in `notes`, if any.
fn kai_note(mut notes: &[u8]) -> Result<Option<(u32, u32)>, Error> {
    let mut found = None;
    while !notes.is_empty() {
        if notes.len() < 12 {
            return Err(Error::BadNote);
        }
        let name_len = u32_at(notes, 0) as usize;
        let desc_len = u32_at(notes, 4) as usize;
        let kind = u32_at(notes, 8);
        let name_end = 12usize.checked_add(name_len).ok_or(Error::BadNote)?;
        let desc_start = name_end.checked_next_multiple_of(4).ok_or(Error::BadNote)?;
        let desc_end = desc_start.checked_add(desc_len).ok_or(Error::BadNote)?;
        let next = desc_end.checked_next_multiple_of(4).ok_or(Error::BadNote)?;
        if desc_end > notes.len() {
            return Err(Error::BadNote);
        }
        if &notes[12..name_end] == KAI_NOTE_NAME && kind == KAI_NOTE_TYPE && desc_len == 8 {
            found = Some((u32_at(notes, desc_start), u32_at(notes, desc_start + 4)));
        }
        notes = &notes[next.min(notes.len())..];
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    const ALLOWED: Range<u64> = 0x1_0000..0x10_0000_0000;

    /// A minimal executable: code at 0x400000 (entry 0x400010), data at 0x401000.
    fn sample() -> Vec<u8> {
        let mut elf = vec![0u8; 0x3000];
        elf[..7].copy_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1]);
        elf[16..18].copy_from_slice(&2u16.to_le_bytes());
        elf[18..20].copy_from_slice(&62u16.to_le_bytes());
        elf[24..32].copy_from_slice(&0x40_0010u64.to_le_bytes());
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&2u16.to_le_bytes());
        set_segment(
            &mut elf,
            0,
            PT_LOAD,
            PF_X | 4,
            0x1000,
            0x40_0000,
            0x100,
            0x100,
        );
        set_segment(
            &mut elf,
            1,
            PT_LOAD,
            PF_W | 4,
            0x2000,
            0x40_1000,
            0x80,
            0x2000,
        );
        elf
    }

    #[allow(clippy::too_many_arguments)]
    fn set_segment(
        elf: &mut [u8],
        index: usize,
        kind: u32,
        flags: u32,
        offset: u64,
        vaddr: u64,
        filesz: u64,
        memsz: u64,
    ) {
        let at = 64 + index * PH_SIZE;
        elf[at..at + 4].copy_from_slice(&kind.to_le_bytes());
        elf[at + 4..at + 8].copy_from_slice(&flags.to_le_bytes());
        elf[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
        elf[at + 16..at + 24].copy_from_slice(&vaddr.to_le_bytes());
        elf[at + 32..at + 40].copy_from_slice(&filesz.to_le_bytes());
        elf[at + 40..at + 48].copy_from_slice(&memsz.to_le_bytes());
    }

    #[test]
    fn accepts_a_well_formed_executable() {
        let image = parse(&sample(), ALLOWED).unwrap();
        assert_eq!(image.entry, 0x40_0010);
        let segments = image.segments();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].file, 0x1000..0x1100);
        assert!(segments[0].execute && !segments[0].write);
        assert_eq!(segments[1].pages(), 0x40_1000..0x40_3000);
        assert_eq!(image.page_count(), 3);
    }

    #[test]
    fn rejects_headers_that_are_not_ours() {
        assert_eq!(parse(&[0; 10], ALLOWED).unwrap_err(), Error::TooShort);
        let mut elf = sample();
        elf[4] = 1;
        assert_eq!(
            parse(&elf, ALLOWED).unwrap_err(),
            Error::NotElf64LittleEndian
        );
        let mut elf = sample();
        elf[16] = 3; // ET_DYN
        assert_eq!(
            parse(&elf, ALLOWED).unwrap_err(),
            Error::NotX86_64Executable
        );
        let mut elf = sample();
        elf[56..58].copy_from_slice(&200u16.to_le_bytes());
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::BadProgramHeaders);
        let mut elf = sample();
        elf[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::BadProgramHeaders);
    }

    #[test]
    fn rejects_dynamic_interpreted_and_tls_programs() {
        for kind in [PT_DYNAMIC, PT_INTERP, PT_TLS] {
            let mut elf = sample();
            set_segment(&mut elf, 1, kind, 4, 0, 0, 0, 0);
            assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::Unsupported);
        }
    }

    #[test]
    fn rejects_segments_outside_the_file_or_the_user_range() {
        let mut elf = sample();
        set_segment(&mut elf, 1, PT_LOAD, PF_W, 0x2F00, 0x40_1000, 0x200, 0x200);
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::SegmentOutsideFile);
        let mut elf = sample();
        set_segment(&mut elf, 1, PT_LOAD, PF_W, u64::MAX, 0x40_1000, 2, 2);
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::SegmentOutsideFile);
        let mut elf = sample();
        set_segment(&mut elf, 1, PT_LOAD, PF_W, 0x2000, 0x40_1000, 0x200, 0x100);
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::SegmentOutsideFile);
        for (vaddr, memsz) in [
            (0, 0x100),
            (0xFFFF_8000_0000_0000, 0x100),
            (0x40_1000, u64::MAX),
            (0xF_FFFF_F000, 0x2000),
        ] {
            let mut elf = sample();
            set_segment(&mut elf, 1, PT_LOAD, PF_W, 0x2000, vaddr, 0, memsz);
            assert_eq!(
                parse(&elf, ALLOWED).unwrap_err(),
                Error::SegmentOutOfRange,
                "{vaddr:#x}+{memsz:#x}"
            );
        }
    }

    #[test]
    fn rejects_writable_code_overlaps_and_bad_entries() {
        let mut elf = sample();
        set_segment(
            &mut elf,
            1,
            PT_LOAD,
            PF_W | PF_X,
            0x2000,
            0x40_1000,
            0x80,
            0x80,
        );
        assert_eq!(
            parse(&elf, ALLOWED).unwrap_err(),
            Error::WritableAndExecutable
        );
        let mut elf = sample();
        set_segment(&mut elf, 1, PT_LOAD, PF_W, 0x2000, 0x40_0800, 0x80, 0x80);
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::Overlap);
        let mut elf = sample();
        elf[24..32].copy_from_slice(&0x40_1000u64.to_le_bytes());
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::BadEntry);
        let mut elf = sample();
        elf[56..58].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::NoSegments);
    }

    /// `sample` with a third program header, a `PT_NOTE` over `notes` at 0x2800.
    fn with_notes(notes: &[u8]) -> Vec<u8> {
        let mut elf = sample();
        elf[56..58].copy_from_slice(&3u16.to_le_bytes());
        set_segment(&mut elf, 2, PT_NOTE, 4, 0x2800, 0, notes.len() as u64, 0);
        elf[0x2800..0x2800 + notes.len()].copy_from_slice(notes);
        elf
    }

    fn note(name: &[u8], kind: u32, desc: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(name);
        out.resize(out.len().next_multiple_of(4), 0);
        out.extend_from_slice(desc);
        out.resize(out.len().next_multiple_of(4), 0);
        out
    }

    fn abi(major: u32, minor: u32) -> Vec<u8> {
        [major.to_le_bytes(), minor.to_le_bytes()].concat()
    }

    #[test]
    fn reads_the_abi_version_from_the_kai_note() {
        assert_eq!(parse(&sample(), ALLOWED).unwrap().abi, None);
        let kai = note(b"KAI\0", 1, &abi(1, 2));
        assert_eq!(parse(&with_notes(&kai), ALLOWED).unwrap().abi, Some((1, 2)));
        // Other notes are skipped, before or after it.
        let gnu = note(b"GNU\0", 3, &[7; 20]);
        let both = [gnu.clone(), kai.clone(), gnu].concat();
        assert_eq!(
            parse(&with_notes(&both), ALLOWED).unwrap().abi,
            Some((1, 2))
        );
        // A note named KAI of another type or size is not it.
        let other = [note(b"KAI\0", 2, &abi(9, 9)), note(b"KAI\0", 1, &[1; 4])].concat();
        assert_eq!(parse(&with_notes(&other), ALLOWED).unwrap().abi, None);
    }

    #[test]
    fn rejects_a_note_that_runs_past_its_segment() {
        let mut kai = note(b"KAI\0", 1, &abi(1, 0));
        kai[4..8].copy_from_slice(&64u32.to_le_bytes());
        assert_eq!(
            parse(&with_notes(&kai), ALLOWED).unwrap_err(),
            Error::BadNote
        );
        let kai = note(b"KAI\0", 1, &abi(1, 0));
        assert_eq!(
            parse(&with_notes(&kai[..10]), ALLOWED).unwrap_err(),
            Error::BadNote
        );
        let mut elf = with_notes(&kai);
        set_segment(&mut elf, 2, PT_NOTE, 4, 0x2FFC, 0, 0x100, 0);
        assert_eq!(parse(&elf, ALLOWED).unwrap_err(), Error::BadNote);
    }
}
