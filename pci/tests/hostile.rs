//! Hostile configuration spaces for the DEV_CFG filter (ADR 0007 sections 5
//! and 12): QEMU-shaped spaces with every pointer, length and ID field at every
//! extreme, loops and out-of-range pointers, seeded mutations and random
//! capability chains, and every offset and width of a write. Each is a verdict
//! and never a panic, and an allowed write holds the harness's promise.

#[path = "hostile/harness.rs"]
mod harness;
#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::XorShift;
use pci::cfg::allow_write;

const WIDTHS: [u8; 6] = [0, 1, 2, 3, 4, 8];
const VALUES: [u32; 6] = [0, 0xFFFF_FFFF, 0x0007, 0x0400, 0x5555_5555, 0x8000_0000];

fn put(space: &mut [u8], at: usize, bytes: &[u8]) {
    space[at..at + bytes.len()].copy_from_slice(bytes);
}

/// QEMU's virtio-net: vendor capabilities, then MSI-X.
fn virtio_net() -> Vec<u8> {
    let mut s = vec![0u8; 256];
    s[6] = 0x10;
    put(&mut s, 4, &[7, 0]);
    s[0x34] = 0x98;
    put(&mut s, 0x98, &[0x11, 0x84, 0x03, 0x80]);
    for (at, next) in [
        (0x84, 0x70),
        (0x70, 0x60),
        (0x60, 0x50),
        (0x50, 0x40),
        (0x40, 0),
    ] {
        put(&mut s, at, &[0x09, next, 0x14, 0x01]);
    }
    s
}

/// QEMU's xhci: MSI, MSI-X, PCIe; extended AER, then a Resizable BAR.
fn xhci() -> Vec<u8> {
    let mut s = vec![0u8; 4096];
    s[6] = 0x10;
    put(&mut s, 4, &[6, 0]);
    s[0x34] = 0x90;
    put(&mut s, 0x90, &[0x05, 0xA0, 0x80, 0x01]);
    put(&mut s, 0xA0, &[0x11, 0x70, 0x08, 0x00]);
    put(&mut s, 0x70, &[0x10, 0x00, 0x02, 0x00]);
    put(
        &mut s,
        0x100,
        &(0x0001u32 | 2 << 16 | 0x140 << 20).to_le_bytes(),
    );
    put(&mut s, 0x140, &(0x0015u32 | 1 << 16).to_le_bytes());
    s
}

fn spaces() -> Vec<Vec<u8>> {
    vec![
        virtio_net(),
        xhci(),
        vec![0; 256],
        vec![0xFF; 256],
        vec![0xFF; 4096],
    ]
}

#[test]
fn every_offset_and_width_of_a_write_to_the_samples() {
    for space in spaces() {
        // Every offset of the header and the standard space, then each dword
        // and its neighbours of the extended space, then past the end.
        let all = (0..0x200u32).chain((0x200..space.len() as u32).step_by(3));
        for offset in all.chain(space.len() as u32..space.len() as u32 + 8) {
            for width in WIDTHS {
                for value in VALUES {
                    harness::check(&space, offset as u16, width, value);
                }
            }
        }
    }
}

#[test]
fn every_pointer_length_and_id_field_at_every_extreme() {
    for sample in [virtio_net(), xhci()] {
        let len = sample.len().min(0x180);
        hostile::sweep_fields(&sample, 0..len, |_, space| {
            let mut space = space.to_vec();
            space.resize(sample.len(), 0);
            // The writes that matter: the command register, the capability
            // bodies and the extended headers, at every width.
            for offset in (0..len as u16).step_by(4) {
                for width in [1u8, 2, 4] {
                    harness::check(&space, offset, width, 0xFFFF_FFFF);
                    harness::check(&space, offset, width, 0);
                }
            }
        });
    }
}

#[test]
fn seeded_mutations_of_the_whole_input() {
    for (i, space) in [virtio_net(), xhci()].iter().enumerate() {
        for offset in [4u16, 0x44, 0x98, 0xA0, 0x100, 0x104, 0x140, 0x144] {
            let input = harness::input(space, 4, offset, 0xFFFF_FFFF);
            hostile::fuzz(
                &input,
                0x5C00 + i as u64 * 16 + u64::from(offset),
                1_500,
                harness::dev_cfg,
            );
            hostile::truncations(&input[..input.len().min(600)], |_, b| harness::dev_cfg(b));
        }
    }
}

#[test]
fn random_capability_chains_never_let_a_protected_byte_change() {
    let mut rng = XorShift::new(0x000D_ECF6);
    for case in 0..1_000 {
        let wide = case % 2 == 0;
        let mut space = vec![0u8; if wide { 4096 } else { 256 }];
        space[6] = if rng.below(8) == 0 { 0 } else { 0x10 };
        // Standard chain: random pointers, IDs among MSI, MSI-X and others.
        space[0x34] = (0x40 + 4 * rng.below(48)) as u8;
        for slot in 0..48 {
            let at = 0x40 + 4 * slot;
            let id = [0x05, 0x11, 0x01, 0x09, 0x10, 0x00][rng.below(6)];
            let next = match rng.below(6) {
                0 => 0,
                1 => rng.below(256) as u8,
                _ => (0x40 + 4 * rng.below(48)) as u8,
            };
            put(
                &mut space,
                at,
                &[id, next, rng.below(256) as u8, rng.below(256) as u8],
            );
        }
        if wide {
            for slot in 0..16 {
                let at = 0x100 + 4 * slot;
                let id = [0x0001u32, 0x0010, 0x0015, 0x0024, 0x000E][rng.below(5)];
                let next = match rng.below(5) {
                    0 => 0,
                    1 => rng.below(4096) as u32,
                    _ => 0x100 + 4 * rng.below(16) as u32,
                };
                put(&mut space, at, &(id | 1 << 16 | next << 20).to_le_bytes());
            }
        }
        // Writes: everywhere the filter might wrongly allow, and random ones.
        for _ in 0..200 {
            let offset = rng.below(space.len()) as u16;
            let width = [1u8, 2, 4][rng.below(3)];
            harness::check(
                &space,
                offset & !(u16::from(width) - 1),
                width,
                rng.next() as u32,
            );
        }
    }
}

#[test]
fn the_filter_is_a_function_of_the_space_alone() {
    // The same write against the same space gives the same verdict, and a
    // verdict never reads past the space it was given.
    for space in spaces() {
        for offset in (0..space.len() as u16).step_by(4) {
            assert_eq!(
                allow_write(&space, offset, 4, 0xDEAD_BEEF),
                allow_write(&space.clone(), offset, 4, 0xDEAD_BEEF)
            );
        }
    }
}

/// Writes the `dev_cfg` target's seeds, one file each, to
/// `$FUZZ_SEED_DIR/dev_cfg/`: a write to the command register, to an MSI-X
/// control word, to a BAR and into an extended header, for each sample space.
#[test]
#[ignore = "writes the fuzz corpus; run by the fuzz workflow"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    let folder = std::path::Path::new(&dir).join("dev_cfg");
    std::fs::create_dir_all(&folder).unwrap();
    let mut n = 0;
    for (name, space, writes) in [
        (
            "virtio-net",
            virtio_net(),
            [
                (4u16, 2u8, 0x0006u32),
                (0x9A, 2, 0xC000),
                (0x10, 4, 0xFFFF_FFFF),
                (0xA4, 4, 0),
            ],
        ),
        (
            "xhci",
            xhci(),
            [
                (4, 2, 0x0406),
                (0xA2, 2, 0xC000),
                (0x14, 4, 0),
                (0x144, 4, 0),
            ],
        ),
    ] {
        for (offset, width, value) in writes {
            let input = harness::input(&space, width, offset, value);
            std::fs::write(folder.join(format!("{name}-{n}")), input).unwrap();
            n += 1;
        }
    }
}
