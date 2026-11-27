//! Hostile executables: every prefix, every header field set to an extreme value
//! and thousands of random mutations of a valid image either parse into an image
//! that keeps every promise the loader relies on, or fail with an error. None
//! panics.

#[path = "hostile/harness.rs"]
mod harness;
#[path = "../../testkit/hostile.rs"]
mod hostile;

use harness::{ALLOWED, check_image};
use hostile::XorShift;
use uelf::{Error, MAX_SEGMENTS, parse};
const PAGE: u64 = 4096;
const PH_SIZE: usize = 56;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

struct Ph {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
}

fn elf(entry: u64, phs: &[Ph], len: usize) -> Vec<u8> {
    let mut elf = vec![0u8; len.max(64 + phs.len() * PH_SIZE)];
    elf[..7].copy_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1]);
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());
    elf[18..20].copy_from_slice(&62u16.to_le_bytes());
    elf[24..32].copy_from_slice(&entry.to_le_bytes());
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    elf[54..56].copy_from_slice(&(PH_SIZE as u16).to_le_bytes());
    elf[56..58].copy_from_slice(&(phs.len() as u16).to_le_bytes());
    for (i, ph) in phs.iter().enumerate() {
        let at = 64 + i * PH_SIZE;
        elf[at..at + 4].copy_from_slice(&ph.kind.to_le_bytes());
        elf[at + 4..at + 8].copy_from_slice(&ph.flags.to_le_bytes());
        elf[at + 8..at + 16].copy_from_slice(&ph.offset.to_le_bytes());
        elf[at + 16..at + 24].copy_from_slice(&ph.vaddr.to_le_bytes());
        elf[at + 32..at + 40].copy_from_slice(&ph.filesz.to_le_bytes());
        elf[at + 40..at + 48].copy_from_slice(&ph.memsz.to_le_bytes());
    }
    elf
}

fn load(flags: u32, offset: u64, vaddr: u64, filesz: u64, memsz: u64) -> Ph {
    Ph {
        kind: PT_LOAD,
        flags,
        offset,
        vaddr,
        filesz,
        memsz,
    }
}

/// Code at 0x400000 (entry 0x400010), read-only data, and data with bss.
fn sample() -> Vec<u8> {
    elf(
        0x40_0010,
        &[
            load(PF_X | PF_R, 0x1000, 0x40_0000, 0x100, 0x100),
            load(PF_R, 0x1800, 0x40_1000, 0x40, 0x40),
            load(PF_W | PF_R, 0x2000, 0x40_2000, 0x80, 0x2000),
        ],
        0x3000,
    )
}

/// The table's end: the bytes every field sweep covers.
const HEADERS_END: usize = 64 + 3 * PH_SIZE;

fn check(data: &[u8]) -> Result<(), Error> {
    let image = parse(data, ALLOWED)?;
    check_image(&image, data);
    Ok(())
}

#[test]
fn the_sample_parses() {
    check(&sample()).unwrap();
}

#[test]
fn every_prefix() {
    let data = sample();
    hostile::truncations(&data, |cut, prefix| {
        let result = check(prefix);
        // The last segment's file bytes end at 0x2080.
        assert_eq!(result.is_ok(), cut >= 0x2080, "cut at {cut}: {result:?}");
    });
}

#[test]
fn every_header_field_set_to_every_extreme() {
    let data = sample();
    hostile::sweep_fields(&data, 0..HEADERS_END, |_, mutated| {
        let _ = check(mutated);
    });
}

#[test]
fn random_mutations_of_the_headers() {
    // Mutations past the headers only change segment contents, so cut the
    // sample's tail off the mutated part and put it back.
    let data = sample();
    for seed in [5, 0xa076_1d64_78bd_642f, 0xe703_7ed1_a0b4_28db] {
        hostile::fuzz(&data[..HEADERS_END], seed, 4000, |head| {
            let mut whole = head.to_vec();
            whole.extend_from_slice(data.get(head.len()..).unwrap_or(&[]));
            let _ = check(&whole);
        });
    }
}

#[test]
fn hostile_tables_and_segments() {
    let code = || load(PF_X | PF_R, 0x1000, 0x40_0000, 0x100, 0x100);
    // A table offset that overflows when the table's size is added, and one past
    // the end.
    for offset in [u64::MAX, u64::MAX - 100, 0x3000 - 55] {
        let mut data = sample();
        data[32..40].copy_from_slice(&offset.to_le_bytes());
        assert_eq!(check(&data), Err(Error::BadProgramHeaders), "{offset:#x}");
    }
    // The most program headers the field holds, and a header size of zero.
    let mut data = sample();
    data[56..58].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(check(&data), Err(Error::BadProgramHeaders));
    let mut data = sample();
    data[54..56].copy_from_slice(&0u16.to_le_bytes());
    assert_eq!(check(&data), Err(Error::BadProgramHeaders));
    // More loadable segments than the image holds.
    let many: Vec<Ph> = (0..MAX_SEGMENTS as u64 + 1)
        .map(|i| load(PF_X | PF_R, 0x1000, 0x40_0000 + i * PAGE, 0x10, 0x10))
        .collect();
    assert_eq!(
        check(&elf(0x40_0000, &many, 0x3000)),
        Err(Error::TooManySegments)
    );
    // A file range that overflows, and memory ranges that overflow or wrap.
    let cases = [
        (
            load(PF_R, u64::MAX, 0x40_1000, 1, 1),
            Error::SegmentOutsideFile,
        ),
        (
            load(PF_R, 1, 0x40_1000, u64::MAX, u64::MAX),
            Error::SegmentOutsideFile,
        ),
        (
            load(PF_R, 0, u64::MAX - 10, 0, 100),
            Error::SegmentOutOfRange,
        ),
        (
            load(PF_R, 0, 0x40_1000, 0, u64::MAX - PAGE + 2),
            Error::SegmentOutOfRange,
        ),
        (load(PF_R, 0, ALLOWED.end, 0, 1), Error::SegmentOutOfRange),
        (
            load(PF_R, 0, ALLOWED.start - 1, 0, 1),
            Error::SegmentOutOfRange,
        ),
        (load(PF_R, 0, 0x40_1000, 0, 0), Error::SegmentOutOfRange),
    ];
    for (ph, expected) in cases {
        let data = elf(0x40_0010, &[code(), ph], 0x3000);
        assert_eq!(check(&data), Err(expected));
    }
    // An entry point one past the end of the code, and inside data only.
    for entry in [0x40_0100, 0x40_1000] {
        let data = elf(
            entry,
            &[code(), load(PF_R, 0x1800, 0x40_1000, 0x40, 0x40)],
            0x3000,
        );
        assert_eq!(check(&data), Err(Error::BadEntry), "{entry:#x}");
    }
}

/// The sample with a fourth program header: a `PT_NOTE` at 0x2800 holding a GNU
/// note and then the "KAI" note with ABI 1.0.
fn with_kai_note() -> (Vec<u8>, std::ops::Range<usize>) {
    let mut notes = Vec::new();
    for (name, kind, desc) in [
        (&b"GNU\0"[..], 3u32, &[7u8; 20][..]),
        (b"KAI\0", 1, &[1, 0, 0, 0, 0, 0, 0, 0]),
    ] {
        notes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        notes.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        notes.extend_from_slice(&kind.to_le_bytes());
        notes.extend_from_slice(name);
        notes.extend_from_slice(desc);
    }
    let range = 0x2800..0x2800 + notes.len();
    let mut data = elf(
        0x40_0010,
        &[
            load(PF_X | PF_R, 0x1000, 0x40_0000, 0x100, 0x100),
            load(PF_R, 0x1800, 0x40_1000, 0x40, 0x40),
            load(PF_W | PF_R, 0x2000, 0x40_2000, 0x80, 0x2000),
            Ph {
                kind: 4,
                flags: PF_R,
                offset: range.start as u64,
                vaddr: 0,
                filesz: notes.len() as u64,
                memsz: 0,
            },
        ],
        0x3000,
    );
    data[range.clone()].copy_from_slice(&notes);
    (data, range)
}

#[test]
fn a_mutated_kai_note_parses_or_fails_and_never_panics() {
    let (data, notes) = with_kai_note();
    assert_eq!(
        parse(&data, ALLOWED).map(|image| image.abi),
        Ok(Some((1, 0)))
    );
    // Every field of both notes at every extreme, and random mutations of them.
    hostile::sweep_fields(&data, notes.clone(), |_, mutated| {
        let _ = check(mutated);
    });
    for seed in [3, 0x9e37_79b9_7f4a_7c15] {
        hostile::fuzz(&data[notes.clone()], seed, 4000, |mutated| {
            let mut whole = data.clone();
            let end = (notes.start + mutated.len()).min(whole.len());
            whole[notes.start..end].copy_from_slice(&mutated[..end - notes.start]);
            let _ = check(&whole);
        });
    }
    // Cut anywhere inside the notes, the segment runs past the file's end.
    hostile::truncations(&data[..notes.end], |cut, prefix| {
        if cut < notes.end {
            assert!(check(prefix).is_err(), "cut at {cut}");
        }
    });
}

// G31: the fuzz target's body (`harness.rs`) over random executables.

#[test]
fn random_bytes_never_panic() {
    let mut rng = XorShift::new(0xe1f);
    for _ in 0..20_000 {
        let len = rng.below(512);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        harness::uelf(&bytes);
    }
}

/// Values near every edge a segment can cross: the allowed range's ends, a
/// page's, and the integer's.
fn edgy(rng: &mut XorShift) -> u64 {
    let near = [
        0,
        1,
        PAGE,
        ALLOWED.start,
        0x40_0000,
        ALLOWED.end,
        ALLOWED.end - PAGE,
        1 << 47,
        u64::MAX - PAGE,
        u64::MAX,
    ];
    let base = near[rng.below(near.len())];
    let jitter = rng.below(2 * PAGE as usize + 1) as u64;
    if rng.below(2) == 0 {
        base.wrapping_add(jitter)
    } else {
        base.wrapping_sub(jitter)
    }
}

/// A valid header over a table of random program headers, so every check of
/// a segment is reached, not only the header's.
#[test]
fn random_program_tables_never_map_outside_allowed() {
    let mut rng = XorShift::new(0x5e9);
    let mut taken = 0;
    for _ in 0..20_000 {
        let count = if rng.below(8) == 0 {
            1 + rng.below(MAX_SEGMENTS + 2)
        } else {
            1 + rng.below(3)
        };
        let phs: Vec<Ph> = (0..count as u64)
            .map(|i| {
                let memsz = if rng.below(4) == 0 {
                    edgy(&mut rng)
                } else {
                    1 + rng.below(0x3000) as u64
                };
                Ph {
                    kind: [
                        PT_LOAD,
                        PT_LOAD,
                        PT_LOAD,
                        PT_LOAD,
                        PT_LOAD,
                        4,
                        6,
                        0x6474_e551,
                    ][rng.below(8)],
                    flags: [PF_R | PF_X, PF_R, PF_R | PF_W, rng.below(8) as u32][rng.below(4)],
                    offset: if rng.below(4) == 0 {
                        edgy(&mut rng)
                    } else {
                        rng.below(0x2f00) as u64
                    },
                    vaddr: if rng.below(4) == 0 {
                        edgy(&mut rng)
                    } else {
                        0x40_0000 + PAGE * (4 * i + rng.below(2) as u64) + rng.below(64) as u64
                    },
                    filesz: rng.below(0x100) as u64 % (memsz.max(1)),
                    memsz,
                }
            })
            .collect();
        let entry = match rng.below(4) {
            0 => edgy(&mut rng),
            _ => phs[rng.below(phs.len())]
                .vaddr
                .wrapping_add(rng.below(64) as u64),
        };
        let data = elf(entry, &phs, 0x3000);
        if let Ok(image) = parse(&data, ALLOWED) {
            check_image(&image, &data);
            taken += 1;
        }
    }
    // Enough tables pass that the image's promises are checked, not only
    // the refusals.
    assert!(taken > 500, "{taken}");
}
