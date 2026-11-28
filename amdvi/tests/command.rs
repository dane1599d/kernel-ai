//! Commands at their bits, and the command buffer ring.

use amdvi::command::{ALL_PAGES, COMMAND_BYTES, Command, CommandError, CommandRing, Pages, opcode};

#[test]
fn completion_wait_stores_its_data_at_its_address() {
    let wait = Command::CompletionWait {
        store: Some((0x1_2345_6788, 0xdead_beef_0bad_cafe)),
        interrupt: false,
        flush: false,
    };
    let words = wait.encode().unwrap();
    assert_eq!(
        words,
        [0x2345_6788 | 1, 1 << 28 | 0x1, 0x0bad_cafe, 0xdead_beef]
    );
    assert_eq!(opcode(words), 1);
    let bytes = wait.to_bytes().unwrap();
    assert_eq!(bytes[..4], (0x2345_6789u32).to_le_bytes());
    let bare = Command::CompletionWait {
        store: None,
        interrupt: true,
        flush: true,
    };
    assert_eq!(bare.encode().unwrap(), [0b110, 1 << 28, 0, 0]);
    for address in [0x1004, 1 << 52] {
        let bad = Command::CompletionWait {
            store: Some((address, 0)),
            interrupt: false,
            flush: false,
        };
        assert_eq!(bad.encode(), Err(CommandError::StoreAddress(address)));
    }
}

#[test]
fn invalidate_devtab_entry_names_the_device() {
    let words = Command::InvalidateDevtabEntry { device: 0x00a8 }
        .encode()
        .unwrap();
    assert_eq!(words, [0x00a8, 2 << 28, 0, 0]);
}

#[test]
fn invalidate_iommu_pages_encodes_one_page_a_range_or_all() {
    let pages = |pages, pde| {
        Command::InvalidateIommuPages {
            domain: 9,
            pages,
            pde,
        }
        .encode()
    };
    assert_eq!(
        pages(Pages::One(0x7_1234_5000), false).unwrap(),
        [0, 3 << 28 | 9, 0x1234_5000, 7]
    );
    // S set; the bits below the size's own are ones.
    let range = |iova, count| pages(Pages::Range { iova, count }, true).unwrap()[2];
    assert_eq!(range(0x2000, 2), 0x2000 | 0b11);
    assert_eq!(range(0x4000, 4), 0x4000 | 0x1000 | 0b11);
    assert_eq!(range(0x10_0000, 256), 0x10_0000 | 0x7_f000 | 0b11);
    assert_eq!(
        pages(Pages::All, true).unwrap(),
        [0, 3 << 28 | 9, 0xffff_f000 | 0b11, 0x7fff_ffff]
    );
    assert_eq!(ALL_PAGES >> 32, 0x7fff_ffff);
    for bad in [
        Pages::One(0x1800),
        Pages::Range {
            iova: 0x2000,
            count: 3,
        },
        Pages::Range {
            iova: 0x2000,
            count: 1,
        },
        Pages::Range {
            iova: 0x2000,
            count: 4,
        },
        Pages::Range {
            iova: 0,
            count: 1 << 62,
        },
    ] {
        assert_eq!(pages(bad, false), Err(CommandError::Pages(bad)));
    }
}

#[test]
fn the_ring_fills_one_short_of_the_head_and_wraps() {
    assert!(CommandRing::new(128).is_none());
    assert!(CommandRing::new(300).is_none());
    assert!(CommandRing::new(65536).is_none());
    let mut ring = CommandRing::new(256).unwrap();
    assert_eq!(ring.bytes(), 4096);
    let mut memory = vec![0u8; 4096];
    let wait = Command::InvalidateDevtabEntry { device: 1 };
    for n in 1..256 {
        ring.push(&mut memory, 0, &wait).unwrap();
        assert_eq!(ring.tail(), n * COMMAND_BYTES as u64);
    }
    assert_eq!(ring.push(&mut memory, 0, &wait), Err(CommandError::Full));
    // The IOMMU read up to 32: two more fit, the second at the end, and the
    // tail wraps to 0.
    ring.push(&mut memory, 32, &wait).unwrap();
    assert_eq!(ring.tail(), 0);
    assert_eq!(memory[4080..4084], 1u32.to_le_bytes());
    ring.push(&mut memory, 32, &wait).unwrap();
    assert_eq!(ring.push(&mut memory, 32, &wait), Err(CommandError::Full));
    // Bits outside 18:4 are not the head's.
    ring.push(&mut memory, 0xffff_0000_0000_0030 | 0xf, &wait)
        .unwrap();
    assert_eq!(
        ring.push(&mut memory, 0x2000, &wait),
        Err(CommandError::Head(0x2000))
    );
    assert_eq!(
        ring.push(&mut [0; 16], 0, &wait),
        Err(CommandError::RingBytes(16))
    );
}
