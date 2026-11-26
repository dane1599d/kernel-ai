//! Hostile inputs: random exit codes and info words, random instruction
//! bytes and random regions. None may panic, and each answer keeps the
//! invariants the run loop relies on.

mod common;

use common::{Contiguous, GIB, MIB, Pool, Rng, guest_ram};
use svm::exit::{Action, Exit, ExitCode, Guest, Insn, Kill, Next, dispatch, intercepts, length};
use svm::npt::{Access, Layout, Region, Tables, translate};
use svm::{Event, Intercepts};

const ROUNDS: usize = 200_000;

fn code(rng: &mut Rng) -> ExitCode {
    ExitCode(match rng.below(4) {
        0 => rng.next(),
        1 => rng.below(0x410),
        2 => 0x060 + rng.below(0x48),
        _ => u64::MAX - rng.below(4),
    })
}

#[test]
fn random_exits_never_panic_and_keep_the_run_loops_invariants() {
    let ram = guest_ram();
    let mut rng = Rng(0x5EED_0019);
    for round in 0..ROUNDS {
        let intercepts = match round % 3 {
            0 => intercepts(),
            1 => Intercepts([u32::MAX; 6]),
            _ => Intercepts(core::array::from_fn(|_| rng.next() as u32)),
        };
        let guest = Guest {
            intercepts,
            ram: &ram,
        };
        let exit = Exit {
            code: code(&mut rng),
            info1: match rng.below(3) {
                0 => rng.below(4),
                1 => rng.next() & 0xFFFF_FFFF,
                _ => rng.next(),
            },
            info2: rng.next() >> rng.below(64),
            nrip: (rng.below(2) == 0).then(|| rng.next()),
        };
        let action = dispatch(&exit, &guest);
        // An unknown code is killed, and only an unknown code is.
        assert_eq!(
            exit.code.name() == "unknown",
            action == Action::Kill(Kill::Unknown(exit.code)),
            "{exit:?}: {action:?}"
        );
        match action {
            // A fault is one EVENTINJ will take.
            Action::Inject(fault) => {
                let field = fault.event.encode();
                assert_eq!(Event::decode(field), Ok(Some(fault.event)));
                assert_eq!(fault.cr2.is_some(), fault.event.vector == 14);
            }
            // RIP moves only to an address the CPU gave.
            Action::Kernel(_, next) | Action::Vmm(_, next) => match next {
                Next::To(rip) => assert!(Some(rip) == exit.nrip || rip == exit.info2),
                Next::Decode(_) => assert_eq!(exit.nrip, None),
                Next::Stay | Next::Emulator => {}
            },
            Action::Kill(_) => {}
        }
    }
}

#[test]
fn random_bytes_never_decode_as_the_wrong_instruction() {
    let insns = [
        Insn::Cpuid,
        Insn::Rdmsr,
        Insn::Wrmsr,
        Insn::Hlt,
        Insn::Xsetbv,
        Insn::Invd,
        Insn::Wbinvd,
        Insn::Rdtsc,
        Insn::Rdtscp,
    ];
    let mut rng = Rng(0xB17E5);
    let mut decoded = 0;
    for round in 0..ROUNDS {
        let insn = insns[round % insns.len()];
        let len = rng.below(20) as usize;
        // Bias towards prefixes and opcode bytes, so decodes happen.
        let pool = [
            0x66, 0x2E, 0xF3, 0x48, 0x0F, 0x01, 0xA2, 0x32, 0x30, 0xF4, 0xD1, 0xF9,
        ];
        let bytes: Vec<u8> = (0..len)
            .map(|_| match rng.below(3) {
                0 => rng.next() as u8,
                _ => pool[rng.below(pool.len() as u64) as usize],
            })
            .collect();
        let long = rng.below(2) == 0;
        if let Some(n) = length(insn, &bytes, long) {
            let n = n as usize;
            let op = insn.opcode();
            assert!(n <= 15 && n <= bytes.len());
            assert_eq!(&bytes[n - op.len()..n], op);
            decoded += 1;
        }
    }
    assert!(decoded > 100, "{decoded}");
}

#[test]
fn random_regions_never_overlap_once_added_and_map_to_their_frames() {
    let mut rng = Rng(0x4E97);
    for _ in 0..2_000 {
        let shm = 64 * MIB;
        let mut layout = Layout::new(4 * GIB, shm).unwrap();
        for _ in 0..12 {
            let page = |rng: &mut Rng, max: u64| {
                let unaligned = rng.below(8) == 0;
                rng.below(max / 4096) * 4096 + if unaligned { rng.below(4096) } else { 0 }
            };
            let r = Region {
                gpa: page(&mut rng, 4 * GIB + GIB),
                bytes: page(&mut rng, 16 * MIB),
                offset: page(&mut rng, shm + MIB),
                access: match rng.below(2) {
                    0 => Access::RAM,
                    _ => Access::READ,
                },
            };
            let _ = layout.add(r);
        }
        let regions: Vec<Region> = layout.regions().map(|(_, r)| r).collect();
        for (i, a) in regions.iter().enumerate() {
            assert!(a.bytes > 0 && a.gpa + a.bytes <= 4 * GIB && a.offset + a.bytes <= shm);
            for b in &regions[i + 1..] {
                assert!(a.gpa + a.bytes <= b.gpa || b.gpa + b.bytes <= a.gpa);
                assert!(a.offset + a.bytes <= b.offset || b.offset + b.bytes <= a.offset);
            }
        }
        let frames = Contiguous {
            base: 0x8_0000_0000,
            bytes: shm,
        };
        let mut pool = Pool::new(512);
        let root = pool.alloc().unwrap();
        layout.map(&frames, &mut pool, root).unwrap();
        for _ in 0..16 {
            let gpa = rng.below(4 * GIB);
            let want = layout
                .find(gpa)
                .map(|r| (frames.base + r.offset + (gpa - r.gpa), r.access));
            assert_eq!(translate(&pool, root, gpa), want, "{gpa:#x}");
        }
    }
}
