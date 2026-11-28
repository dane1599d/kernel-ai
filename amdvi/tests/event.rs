//! Event log entries decoded, and the log ring drained, from words the
//! IOMMU wrote: any words decode, and any head and tail registers are read
//! or refused.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use amdvi::event::{EVENT_BYTES, Event, EventError, EventLog, Flags};
use amdvi::regs;

fn entry(words: [u32; 4]) -> [u8; EVENT_BYTES] {
    let mut bytes = [0; EVENT_BYTES];
    for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

#[test]
fn an_io_page_fault_names_its_device_domain_access_and_address() {
    let flags = 1 << 20 | 1 << 21 | 1 << 22;
    let words = [0x0018, 2 << 28 | flags | 0x0005, 0x1234_5000, 0x1];
    let Event::IoPageFault {
        device,
        domain,
        flags,
        address,
    } = Event::from_bytes(entry(words))
    else {
        panic!("not a page fault");
    };
    assert_eq!((device, domain, address), (0x18, 5, 0x1_1234_5000));
    assert!(flags.present() && flags.write() && flags.permission());
    assert!(!flags.guest() && !flags.execute() && !flags.user() && !flags.interrupt());
    assert!(!flags.reserved_bits() && !flags.translation_request());
}

#[test]
fn each_flag_and_code_is_its_own() {
    let names: [fn(Flags) -> bool; 9] = [
        Flags::guest,
        Flags::execute,
        Flags::user,
        Flags::interrupt,
        Flags::present,
        Flags::write,
        Flags::permission,
        Flags::reserved_bits,
        Flags::translation_request,
    ];
    for n in 0..names.len() {
        for (m, other) in names.iter().enumerate() {
            assert_eq!(other(Flags(1 << n)), n == m, "flag {n} as {m}");
        }
    }
    assert_eq!(Flags(0b11 << 9).kind(), 3);
    assert_eq!(Flags(1 << 9).kind(), 1);
    let codes: Vec<_> = (0..16u32)
        .map(|code| Event::decode([0x10, code << 28 | 0x20, 0x30, 0x40]))
        .collect();
    assert!(matches!(codes[0], Event::Other { code: 0, .. }));
    assert!(matches!(
        codes[1],
        Event::IllegalDevTableEntry {
            device: 0x10,
            address: 0x40_0000_0030,
            ..
        }
    ));
    assert!(matches!(
        codes[2],
        Event::IoPageFault {
            device: 0x10,
            domain: 0x20,
            ..
        }
    ));
    assert!(matches!(
        codes[3],
        Event::DevTabHardwareError { device: 0x10, .. }
    ));
    assert!(matches!(
        codes[4],
        Event::PageTabHardwareError {
            device: 0x10,
            domain: 0x20,
            ..
        }
    ));
    assert!(matches!(
        codes[5],
        Event::IllegalCommand {
            address: 0x40_0000_0030
        }
    ));
    assert!(matches!(codes[6], Event::CommandHardwareError { .. }));
    assert!(matches!(
        codes[7],
        Event::IotlbInvTimeout { device: 0x10, .. }
    ));
    assert!(matches!(
        codes[8],
        Event::InvalidDeviceRequest { device: 0x10, .. }
    ));
    for (code, event) in codes.iter().enumerate().skip(9) {
        assert_eq!(
            *event,
            Event::Other {
                code: code as u8,
                words: [0x10, (code as u32) << 28 | 0x20, 0x30, 0x40]
            }
        );
    }
}

#[test]
fn a_drain_reads_from_head_to_tail_across_the_end() {
    let log = EventLog::new(256).unwrap();
    let mut ring = vec![0u8; log.bytes()];
    for (n, at) in [4064usize, 4080, 0, 16].into_iter().enumerate() {
        ring[at..at + 16].copy_from_slice(&entry([n as u32, 7 << 28, 0, 0]));
    }
    let (events, head) = log.drain(&ring, 4064, 32).unwrap();
    assert_eq!(head, 32);
    let devices: Vec<_> = events
        .iter()
        .map(|e| match e {
            Event::IotlbInvTimeout { device, .. } => *device,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(devices, [0, 1, 2, 3]);
    assert_eq!(log.drain(&ring, 32, 32).unwrap().0, []);
    assert_eq!(log.drain(&ring, 0, 4096), Err(EventError::Pointer(4096)));
    assert_eq!(log.drain(&ring[..16], 0, 0), Err(EventError::RingBytes(16)));
}

/// Random rings and random head and tail registers: read or refused, never
/// a panic, and never more entries than the ring holds.
#[test]
fn hostile_logs_are_read_or_refused() {
    let mut rng = hostile::XorShift::new(0x48882);
    for _ in 0..2_000 {
        let entries = 256 << rng.below(4);
        let log = EventLog::new(entries).unwrap();
        let ring: Vec<u8> = (0..log.bytes()).map(|_| rng.next() as u8).collect();
        let pointer = |rng: &mut hostile::XorShift| match rng.below(3) {
            0 => rng.next(),
            1 => rng.next() & 0x7_fff0,
            _ => (rng.below(entries as usize) * EVENT_BYTES) as u64,
        };
        let (head, tail) = (pointer(&mut rng), pointer(&mut rng));
        match log.drain(&ring, head, tail) {
            Ok((events, new_head)) => {
                assert!(events.len() < entries as usize);
                assert_eq!(new_head, tail & 0x7_fff0);
            }
            Err(EventError::Pointer(p)) => assert!(p >= log.bytes() as u64),
            Err(e) => panic!("{e:?}"),
        }
    }
    for _ in 0..100_000 {
        let _ = Event::decode([
            rng.next() as u32,
            rng.next() as u32,
            rng.next() as u32,
            rng.next() as u32,
        ]);
    }
}

#[test]
fn the_base_registers_hold_address_and_size() {
    assert_eq!(
        regs::device_table_base(0x10_0000, 2 << 20),
        Some(0x10_0000 | 511)
    );
    assert_eq!(regs::device_table_base(0x10_0000, 4096), Some(0x10_0000));
    for (phys, bytes) in [
        (0x10_0800, 4096),
        (0x10_0000, 0),
        (0x10_0000, 100),
        (0x10_0000, 4 << 20),
        (1 << 52, 4096),
    ] {
        assert_eq!(
            regs::device_table_base(phys, bytes),
            None,
            "{phys:#x} {bytes}"
        );
    }
    assert_eq!(regs::ring_base(0x20_0000, 256), Some(0x20_0000 | 8 << 56));
    assert_eq!(
        regs::ring_base(0x20_0000, 32768),
        Some(0x20_0000 | 15 << 56)
    );
    for entries in [0, 128, 300, 65536] {
        assert_eq!(regs::ring_base(0x20_0000, entries), None);
    }
    assert_eq!(regs::ring_base(0x20_0010, 256), None);
}
