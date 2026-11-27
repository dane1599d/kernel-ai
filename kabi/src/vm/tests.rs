//! `kabi::vm` 1: every record and every exit kind round-trips, and each rule in
//! the module doc refuses what it names.

use super::*;

/// A vCPU as the Linux 64-bit boot protocol takes it: long mode, a GDT with
/// `__BOOT_CS` (0x10) and `__BOOT_DS` (0x18), RSI at the zero page.
pub(crate) fn boot_regs() -> Regs {
    let mut gpr = [0; 16];
    gpr[6] = 0x7000;
    Regs {
        vcpu: 0,
        gpr,
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

pub(crate) fn every_call() -> [Call; 8] {
    [
        Call::Create(Create {
            vcpus: 4,
            ram_mib: 1536,
            flags: 0,
        }),
        Call::Create(Create {
            vcpus: 1,
            ram_mib: 2,
            flags: 0,
        }),
        Call::SetRegs(boot_regs()),
        Call::Run,
        Call::Irq(Irq::Pin { pin: 23, level: 1 }),
        Call::Irq(Irq::Msi {
            address: 0xFEE0_0000,
            data: 0x41,
        }),
        Call::Stop,
        Call::Destroy,
    ]
}

pub(crate) fn every_exit() -> [ExitEntry; 9] {
    let at = |seq, vcpu, exit| ExitEntry { seq, vcpu, exit };
    [
        at(
            0,
            0,
            Exit::Mmio {
                address: 0xFEC0_0000,
                size: 4,
                write: None,
            },
        ),
        at(
            1,
            3,
            Exit::Mmio {
                address: (1 << 52) - 8,
                size: 8,
                write: Some(u64::MAX),
            },
        ),
        at(
            2,
            1,
            Exit::Pio {
                port: 0x3F8,
                size: 1,
                write: Some(b'K'.into()),
            },
        ),
        at(
            3,
            0,
            Exit::Pio {
                port: 0xFFFC,
                size: 4,
                write: None,
            },
        ),
        at(
            4,
            2,
            Exit::Doorbell {
                address: 0xE000_3000,
                size: 2,
                data: 1,
            },
        ),
        at(
            5,
            0,
            Exit::Doorbell {
                address: 0xE000_3004,
                size: 4,
                data: 0xFFFF_FFFF,
            },
        ),
        at(6, 0, Exit::Shutdown),
        at(u64::MAX, 3, Exit::TripleFault),
        at(
            9,
            0,
            Exit::Pio {
                port: 0xCFC,
                size: 2,
                write: Some(0xBEEF),
            },
        ),
    ]
}

fn bytes(call: &Call) -> ([u8; MAX_CALL_BYTES], usize) {
    let mut out = [0; MAX_CALL_BYTES];
    let len = call.encode(&mut out);
    (out, len)
}

#[test]
fn every_record_round_trips_at_its_own_length() {
    for call in every_call() {
        let (out, len) = bytes(&call);
        let want = match call.op() {
            op::CREATE => CREATE_BYTES,
            op::SET_REGS => REGS_BYTES,
            op::IRQ => IRQ_BYTES,
            _ => BARE_BYTES,
        };
        assert_eq!(len, want, "{call:?}");
        assert_eq!(decode(&out[..len]), Ok(call));
        // One byte more or less is refused, never read as something else.
        let mut long = alloc::vec::Vec::from(&out[..len]);
        long.push(0);
        assert_eq!(decode(&long), Err(Refused::Length), "{call:?}");
        assert_eq!(decode(&out[..len - 1]), Err(Refused::Length), "{call:?}");
    }
}

#[test]
fn every_exit_round_trips_through_the_request_and_its_bytes() {
    for entry in every_exit() {
        assert_eq!(ExitEntry::from_request(&entry.request()), Ok(entry));
        assert_eq!(ExitEntry::from_bytes(&entry.to_bytes()), Ok(entry));
        assert_eq!(entry.request().buffer, 0);
    }
}

#[test]
fn another_version_or_operation_is_refused() {
    let (mut out, len) = bytes(&Call::Run);
    out[0] = 2;
    assert_eq!(decode(&out[..len]), Err(Refused::Version));
    let (mut out, len) = bytes(&Call::Run);
    for op in [0u16, 7, u16::MAX] {
        out[2..4].copy_from_slice(&op.to_le_bytes());
        assert_eq!(decode(&out[..len]), Err(Refused::Op));
    }
    assert_eq!(decode(&[]), Err(Refused::Length));
    assert_eq!(decode(&[1, 0, 3]), Err(Refused::Length));
}

#[test]
fn every_reserved_byte_is_refused_when_set() {
    for call in every_call() {
        let (out, len) = bytes(&call);
        for at in 4..len {
            let mut bad = out;
            bad[at] ^= 0x80;
            if let Ok(read) = decode(&bad[..len]) {
                // A used field changed: then the record says something else.
                assert_ne!(read, call, "byte {at} of {call:?} was ignored");
            }
        }
    }
}

#[test]
fn create_is_held_to_the_v1_defaults() {
    let create = |vcpus, ram_mib, flags| {
        Call::Create(Create {
            vcpus,
            ram_mib,
            flags,
        })
        .check()
    };
    assert_eq!(create(4, 1536, 0), Ok(()));
    for (vcpus, ram, flags) in [
        (0, 2, 0),
        (5, 2, 0),
        (1, 0, 0),
        (1, 3, 0),
        (1, 1538, 0),
        (1, 2, 1),
    ] {
        assert_eq!(
            create(vcpus, ram, flags),
            Err(Refused::Range),
            "{vcpus} {ram} {flags}"
        );
    }
}

#[test]
fn set_regs_takes_long_mode_and_only_the_bits_boot_needs() {
    let refused = |edit: fn(&mut Regs)| {
        let mut r = boot_regs();
        edit(&mut r);
        Call::SetRegs(r).check()
    };
    assert_eq!(Call::SetRegs(boot_regs()).check(), Ok(()));
    let edits: [fn(&mut Regs); 16] = [
        |r| r.vcpu = MAX_VCPUS,
        |r| r.cr0 &= !(1 << 31),
        |r| r.cr0 |= 1 << 1 << 20,
        |r| r.cr4 = 0,
        |r| r.cr4 |= 1 << 17,
        |r| r.efer &= !(1 << 10),
        |r| r.efer |= 1 << 12,
        |r| r.rflags = 0,
        |r| r.rflags |= 1 << 3,
        |r| r.cr3 |= 0x800,
        |r| r.cr3 = 1 << 52,
        |r| r.rip = 1 << 47,
        |r| r.gdt_base = 0x8000_0000_0000,
        |r| r.cs = 0,
        |r| r.ds = 0x1B,
        |r| r.cs = 0x20,
    ];
    for (i, edit) in edits.into_iter().enumerate() {
        assert_eq!(refused(edit), Err(Refused::Range), "edit {i}");
    }
    // A canonical upper-half RIP is taken.
    let mut r = boot_regs();
    r.rip = 0xFFFF_FFFF_8100_0000;
    assert_eq!(Call::SetRegs(r).check(), Ok(()));
}

#[test]
fn irq_takes_a_pin_or_an_msi_in_range() {
    let check = |irq| Call::Irq(irq).check();
    assert_eq!(check(Irq::Pin { pin: 24, level: 0 }), Err(Refused::Range));
    assert_eq!(check(Irq::Pin { pin: 0, level: 2 }), Err(Refused::Range));
    let msi = |address, data| check(Irq::Msi { address, data });
    assert_eq!(msi(0xFEEF_FFFF, 0xFFFF), Ok(()));
    assert_eq!(msi(0xFEDF_FFFF, 0), Err(Refused::Range));
    assert_eq!(msi(0xFEF0_0000, 0), Err(Refused::Range));
    assert_eq!(msi(0xFEE0_0000, 0x1_0000), Err(Refused::Range));
    // A kind byte other than pin or MSI is no operation.
    let (mut out, len) = bytes(&Call::Irq(Irq::Pin { pin: 1, level: 0 }));
    out[8] = 3;
    assert_eq!(decode(&out[..len]), Err(Refused::Op));
}

#[test]
fn an_exit_out_of_range_is_refused() {
    let base = every_exit()[0].request();
    let refused = |edit: fn(&mut Request)| {
        let mut r = base;
        edit(&mut r);
        ExitEntry::from_request(&r)
    };
    type Edit = (fn(&mut Request), Refused);
    let edits: [Edit; 12] = [
        (|r| r.op = 0, Refused::Op),
        (|r| r.op = 6, Refused::Op),
        (|r| r.buffer = 1, Refused::Reserved),
        (|r| r._r = 1, Refused::Reserved),
        (|r| r.args[2] = 1, Refused::Reserved),
        (|r| r.flags = 2, Refused::Reserved),
        (|r| r.args[0] = MAX_VCPUS.into(), Refused::Range),
        (|r| r.len = 3, Refused::Range),
        (|r| r.len = 1 << 20, Refused::Range),
        (|r| r.offset = 1 << 52, Refused::Range),
        // A read carries no data.
        (|r| r.args[1] = 1, Refused::Reserved),
        // A write's data fits its width.
        (
            |r| {
                r.flags = vmexit::WRITE;
                r.args[1] = 1 << 32;
            },
            Refused::Range,
        ),
    ];
    for (i, (edit, why)) in edits.into_iter().enumerate() {
        assert_eq!(refused(edit), Err(why), "edit {i}");
    }
    let port = |offset, len| {
        let mut r = every_exit()[3].request();
        r.offset = offset;
        r.len = len;
        ExitEntry::from_request(&r)
    };
    assert!(port(0xFFFC, 4).is_ok());
    assert_eq!(port(0xFFFD, 4), Err(Refused::Range));
    assert_eq!(port(u64::MAX, 4), Err(Refused::Range));
    assert_eq!(port(0, 8), Err(Refused::Range));
    // A doorbell is a write of 2 or 4 bytes.
    let mut bell = every_exit()[4].request();
    bell.flags = 0;
    bell.args[1] = 0;
    assert_eq!(ExitEntry::from_request(&bell), Err(Refused::Range));
    bell.flags = vmexit::WRITE;
    bell.len = 1;
    assert_eq!(ExitEntry::from_request(&bell), Err(Refused::Range));
    // A shutdown carries only its vCPU.
    let mut down = every_exit()[6].request();
    down.offset = 1;
    assert_eq!(ExitEntry::from_request(&down), Err(Refused::Reserved));
}

#[test]
fn a_reply_is_checked_against_its_exit() {
    let read = every_exit()[0];
    let write = every_exit()[2];
    let rsp = |tag, status, len| Response { tag, status, len };
    let slot = 0xDEAD_BEEFu64.to_le_bytes();
    assert_eq!(
        check_reply(&read, &rsp(0, 0, 4), slot),
        Ok(Ok(Some(0xDEAD_BEEF)))
    );
    // Bytes past the read's size are zero.
    let wide = 0x1_DEAD_BEEFu64.to_le_bytes();
    assert_eq!(
        check_reply(&read, &rsp(0, 0, 4), wide),
        Err(Refused::Reserved)
    );
    assert_eq!(
        check_reply(&read, &rsp(0, 0, 2), slot),
        Err(Refused::Length)
    );
    assert_eq!(check_reply(&read, &rsp(1, 0, 4), slot), Err(Refused::Range));
    assert_eq!(check_reply(&write, &rsp(2, 0, 0), [0; 8]), Ok(Ok(None)));
    assert_eq!(
        check_reply(&write, &rsp(2, 0, 1), [0; 8]),
        Err(Refused::Reserved)
    );
    // A status is a negative kabi::Error, with no length.
    let invalid = -(crate::Error::Invalid as i32);
    assert_eq!(
        check_reply(&read, &rsp(0, invalid, 0), slot),
        Ok(Err(crate::Error::Invalid))
    );
    assert_eq!(
        check_reply(&read, &rsp(0, invalid, 4), slot),
        Err(Refused::Reserved)
    );
    for status in [1, -99, i32::MIN, i32::MAX] {
        assert_eq!(
            check_reply(&read, &rsp(0, status, 0), slot),
            Err(Refused::Range)
        );
    }
}

#[test]
fn the_data_slot_is_the_requests_ring_slot() {
    assert_eq!(data_slot(0, 8), Some(0));
    assert_eq!(data_slot(9, 8), Some(8));
    assert_eq!(data_slot(u64::MAX, 4096), Some(4095 * 8));
    for entries in [0, 4, 12, 8192, u32::MAX] {
        assert_eq!(data_slot(1, entries), None, "{entries}");
    }
}
