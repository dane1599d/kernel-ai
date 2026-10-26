//! Hostile input for format 1's readers (ADR 0004; disks become untrusted input
//! in P7): mutated superblocks and pointers, every field at every extreme value,
//! and rings full of garbage. Each must decode or be refused, never panic, and
//! whatever decodes must encode back to itself.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::{XorShift, fuzz, sweep_fields};
use kaifs::format::{self, BlockPtr, FLAG_SEALED, KIND_NODE, KIND_RECORD, PTR_BYTES, Superblock};
use kaifs::{BLOCK, Block, BlockDev, CsumKind, MemDisk};

fn block_of(bytes: &[u8]) -> Block {
    let mut block = [0; BLOCK];
    let n = bytes.len().min(BLOCK);
    block[..n].copy_from_slice(&bytes[..n]);
    block
}

/// Gives a mutated superblock a good checksum again, so the field checks behind
/// it run.
fn resign(block: &mut Block) {
    let sum = CsumKind::Seahash64.sum(&block[..BLOCK - 32]);
    block[BLOCK - 32..].copy_from_slice(&sum);
}

fn sample_ptr() -> BlockPtr {
    BlockPtr {
        addr: 1234,
        birth: 56,
        blocks: 32,
        kind: KIND_RECORD,
        level: 0,
        flags: FLAG_SEALED,
        csum: [0xA5; 32],
    }
}

fn sample() -> Block {
    let mut sb = Superblock::new(*b"hostile kaifs fs", 1 << 20, 99);
    sb.volumes = BlockPtr {
        kind: KIND_NODE,
        blocks: 1,
        level: 3,
        flags: 0,
        ..sample_ptr()
    };
    sb.alloc = BlockPtr {
        kind: format::KIND_ALLOC,
        blocks: 1,
        flags: 0,
        ..sample_ptr()
    };
    sb.commit_time = 1_798_000_000_000_000_000;
    sb.label[..4].copy_from_slice(b"home");
    sb.encode()
}

fn decode_is_total(block: &Block) {
    if let Ok(sb) = Superblock::decode(block) {
        assert_eq!(Superblock::decode(&sb.encode()), Ok(sb));
        let _ = sb.rules();
    }
}

#[test]
fn mutated_superblocks_never_panic_and_what_decodes_round_trips() {
    let sample = sample();
    fuzz(&sample, 1, 20_000, |bytes| {
        let mut block = block_of(bytes);
        decode_is_total(&block);
        resign(&mut block);
        decode_is_total(&block);
    });
}

#[test]
fn every_superblock_field_at_every_extreme_value() {
    let sample = sample();
    sweep_fields(&sample, 0..408, |_, bytes| {
        let mut block = block_of(bytes);
        resign(&mut block);
        decode_is_total(&block);
    });
}

fn pointer_is_total(bytes: &[u8]) {
    let mut raw = [0; PTR_BYTES];
    let n = bytes.len().min(PTR_BYTES);
    raw[..n].copy_from_slice(&bytes[..n]);
    if let Ok(ptr) = BlockPtr::decode(&raw) {
        assert_eq!(BlockPtr::decode(&ptr.encode()), Ok(ptr));
    }
    let _ = BlockPtr::decode_at(bytes, 0);
    let _ = BlockPtr::decode_at(bytes, bytes.len().saturating_sub(8));
}

#[test]
fn mutated_pointers_never_panic_and_what_decodes_round_trips() {
    let sample = sample_ptr().encode();
    fuzz(&sample, 2, 50_000, pointer_is_total);
    sweep_fields(&sample, 0..PTR_BYTES, |_, bytes| pointer_is_total(bytes));
}

#[test]
fn rings_of_garbage_never_panic() {
    let mut rng = XorShift::new(3);
    let blocks = 4096;
    for _ in 0..300 {
        let mut disk = MemDisk::new(blocks);
        for addr in format::all_slots(blocks) {
            let mut block = match rng.below(4) {
                0 => [0; BLOCK],
                1 => block_of(&hostile::mutate(&mut rng, &sample())),
                _ => {
                    let mut sb =
                        Superblock::new([rng.below(3) as u8; 16], blocks, rng.below(40) as u64);
                    sb.incompat = (rng.below(8) == 0) as u64;
                    sb.encode()
                }
            };
            if rng.below(2) == 0 {
                resign(&mut block);
            }
            disk.write(addr, &block).unwrap();
        }
        if let Ok(found) = format::read_rings(&mut disk) {
            assert!(!found.is_empty());
            assert!(found.windows(2).all(|w| w[0].generation > w[1].generation));
            assert!(found.iter().all(|sb| sb.uuid == found[0].uuid));
        }
    }
}

/// A chain head of `which` kind, then the same head with its bytes mutated and
/// its pointer's checksum made good, so the replay's own checks run: each must
/// load or be refused, never panic, and what loads must stay inside the disk.
#[test]
fn mutated_allocator_blocks_never_panic() {
    use kaifs::allocator::Allocator;
    let blocks = 1 << 14;
    let area = format::data_area(blocks);
    let kind = CsumKind::Seahash64;
    for txns in [1u64, 5] {
        let mut disk = MemDisk::new(blocks);
        let mut alloc = Allocator::new(area);
        let mut head = BlockPtr::NULL;
        for txn in 1..=txns {
            let run = alloc.alloc((txn % 4) as u8).unwrap();
            alloc.free_later(txn, run);
            head = alloc.persist(&mut disk, kind, txn).unwrap();
            alloc.committed(txn);
        }
        let mut sample = [0; BLOCK];
        disk.read(head.addr, &mut sample).unwrap();
        let mut rng = XorShift::new(txns);
        for _ in 0..5_000 {
            let block = block_of(&hostile::mutate(&mut rng, &sample));
            let mut copy = disk.clone();
            copy.write(head.addr, &block).unwrap();
            let ptr = BlockPtr {
                csum: kind.sum(&block),
                ..head
            };
            if let Ok(loaded) = Allocator::load(&mut copy, kind, &ptr, txns + rng.below(3) as u64) {
                let state = loaded.state();
                assert!(
                    state
                        .free
                        .iter()
                        .all(|&(s, l)| s >= area.0 && s + l <= area.1)
                );
                assert!(
                    state
                        .pending
                        .iter()
                        .all(|&(_, s, l)| s >= area.0 && s + l <= area.1)
                );
            }
        }
    }
}
