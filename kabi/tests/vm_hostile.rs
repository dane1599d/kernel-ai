//! Hostile `kabi::vm` 1 records and `VMEXIT` entries: random bytes, every
//! truncation, every field position with extreme values, and every version and
//! kind are refused with an error or taken as exactly what they encode, never
//! a panic.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::{XorShift, fuzz, sweep_fields, truncations};
use kabi::ring::{REQUEST_BYTES, Response};
use kabi::vm::{
    Call, Create, Exit, ExitEntry, Irq, MAX_CALL_BYTES, Regs, check_reply, data_slot, decode, op,
};

fn regs() -> Regs {
    Regs {
        vcpu: 0,
        gpr: [7; 16],
        rip: 0x20_0000,
        rflags: 0x2,
        cr0: 1 | (1 << 31),
        cr3: 0x9000,
        cr4: 1 << 5,
        efer: (1 << 8) | (1 << 10),
        gdt_base: 0x500,
        gdt_limit: 0x1F,
        idt_base: 0,
        idt_limit: 0,
        cs: 0x10,
        ds: 0x18,
    }
}

fn samples() -> Vec<Vec<u8>> {
    [
        Call::Create(Create {
            vcpus: 2,
            ram_mib: 512,
            flags: 0,
        }),
        Call::SetRegs(regs()),
        Call::Run,
        Call::Irq(Irq::Pin { pin: 4, level: 1 }),
        Call::Irq(Irq::Msi {
            address: 0xFEE0_1000,
            data: 0x30,
        }),
        Call::Stop,
        Call::Destroy,
    ]
    .iter()
    .map(|call| {
        let mut out = [0; MAX_CALL_BYTES];
        let len = call.encode(&mut out);
        out[..len].to_vec()
    })
    .collect()
}

fn exits() -> Vec<ExitEntry> {
    let at = |seq, exit| ExitEntry { seq, vcpu: 1, exit };
    vec![
        at(
            1,
            Exit::Mmio {
                address: 0xFEC0_0010,
                size: 4,
                write: Some(0x1234),
            },
        ),
        at(
            2,
            Exit::Pio {
                port: 0x3F8,
                size: 1,
                write: None,
            },
        ),
        at(
            3,
            Exit::Doorbell {
                address: 0xE000_3000,
                size: 2,
                data: 3,
            },
        ),
        at(4, Exit::Shutdown),
        at(5, Exit::TripleFault),
    ]
}

/// What `decode` took must be one in-range record that encodes back to
/// exactly these bytes.
fn taken_only_as_itself(bytes: &[u8]) {
    let Ok(call) = decode(bytes) else { return };
    assert_eq!(call.check(), Ok(()));
    let mut out = [0; MAX_CALL_BYTES];
    let len = call.encode(&mut out);
    assert_eq!(&out[..len], bytes, "{call:?}");
}

/// The same for an exit entry.
fn exit_taken_only_as_itself(bytes: &[u8]) {
    let Ok(entry) = ExitEntry::from_bytes(bytes) else {
        return;
    };
    assert_eq!(&entry.to_bytes()[..], bytes, "{entry:?}");
    // Its reply check never panics either, on any slot.
    for status in [0, -3, i32::MIN] {
        for len in [0, 1, 4, 8, u32::MAX] {
            let reply = Response {
                tag: entry.seq,
                status,
                len,
            };
            let _ = check_reply(&entry, &reply, bytes[..8].try_into().unwrap());
        }
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = XorShift::new(0x766d);
    for _ in 0..20_000 {
        let len = rng.below(MAX_CALL_BYTES + 16);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        taken_only_as_itself(&bytes);
        exit_taken_only_as_itself(&bytes);
        // Random bytes behind a valid header reach every operation's checks.
        if len >= 8 {
            let mut framed = bytes.clone();
            framed[0..2].copy_from_slice(&1u16.to_le_bytes());
            framed[2..4].copy_from_slice(&(1 + rng.below(6) as u16).to_le_bytes());
            framed[4..8].fill(0);
            taken_only_as_itself(&framed);
        }
        let _ = data_slot(rng.next(), rng.next() as u32);
    }
}

#[test]
fn mutated_records_and_exits_never_panic() {
    for (i, sample) in samples().iter().enumerate() {
        fuzz(sample, 0x5000 + i as u64, 5_000, taken_only_as_itself);
    }
    for (i, entry) in exits().iter().enumerate() {
        fuzz(
            &entry.to_bytes(),
            0x6000 + i as u64,
            5_000,
            exit_taken_only_as_itself,
        );
    }
}

#[test]
fn every_truncation_is_refused() {
    for sample in samples() {
        truncations(&sample, |cut, bytes| {
            if cut < sample.len() {
                assert!(decode(bytes).is_err(), "{cut} of {sample:?}");
            } else {
                assert!(decode(bytes).is_ok());
            }
        });
    }
    for entry in exits() {
        let bytes = entry.to_bytes();
        truncations(&bytes, |cut, short| {
            assert_eq!(ExitEntry::from_bytes(short).is_ok(), cut == REQUEST_BYTES);
        });
        let mut long = bytes.to_vec();
        long.push(0);
        assert!(ExitEntry::from_bytes(&long).is_err());
    }
}

#[test]
fn every_field_with_extreme_values_never_panics() {
    for sample in samples() {
        sweep_fields(&sample, 0..sample.len(), |_, bytes| {
            taken_only_as_itself(bytes)
        });
    }
    for entry in exits() {
        let bytes = entry.to_bytes();
        sweep_fields(&bytes, 0..bytes.len(), |_, b| exit_taken_only_as_itself(b));
    }
}

#[test]
fn every_version_and_kind_but_v1_s_own_is_refused() {
    let known_ops = [
        op::CREATE,
        op::SET_REGS,
        op::RUN,
        op::IRQ,
        op::STOP,
        op::DESTROY,
    ];
    for sample in samples() {
        for version in 0..=u16::MAX {
            let mut bytes = sample.clone();
            bytes[0..2].copy_from_slice(&version.to_le_bytes());
            assert_eq!(decode(&bytes).is_ok(), version == 1, "version {version}");
        }
        for kind in 0..=u16::MAX {
            let mut bytes = sample.clone();
            bytes[2..4].copy_from_slice(&kind.to_le_bytes());
            if !known_ops.contains(&kind) {
                assert!(decode(&bytes).is_err(), "op {kind}");
            }
        }
    }
    for entry in exits() {
        for kind in 0..=u16::MAX {
            let mut bytes = entry.to_bytes();
            bytes[8..10].copy_from_slice(&kind.to_le_bytes());
            if !(1..=5).contains(&kind) {
                assert!(ExitEntry::from_bytes(&bytes).is_err(), "kind {kind}");
            }
        }
    }
}
