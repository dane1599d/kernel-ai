//! EPT entries against vol. 3C 29.3.2 (tables 29-1 to 29-6), the
//! misconfigurations of 29.3.3.1, and the EPT pointer of 25.6.11.

use crate::ept::{
    ACCESSED, ADDRESS, Access, BadEntry, EXECUTE, EptEntry, Eptp, IGNORE_PAT, Kind, LARGE,
    MAX_PHYSICAL, MemoryType, READ, SUPPRESS_VE, Size, Table, USER_EXECUTE, WRITE, cap, indices,
};
use core::mem::{align_of, size_of};

#[test]
fn a_table_entry_grants_everything_and_points_at_a_page() {
    let entry = EptEntry::table(0x0012_3000).unwrap();
    assert_eq!(entry.0, 0x0012_3000 | READ | WRITE | EXECUTE);
    assert_eq!(entry.check(4, 52), Ok(Kind::Table));
    assert_eq!(entry.check(3, 52), Ok(Kind::Table));
    assert_eq!(entry.address(), 0x0012_3000);
    assert_eq!(
        EptEntry::table(0x0012_3008),
        Err(BadEntry::Address(0x0012_3008))
    );
    assert_eq!(
        EptEntry::table(MAX_PHYSICAL + 1),
        Err(BadEntry::Address(MAX_PHYSICAL + 1))
    );
}

#[test]
fn a_leaf_carries_its_rights_type_and_size() {
    let ram = EptEntry::leaf(
        0x0040_0000,
        Size::Page2M,
        Access::RAM,
        MemoryType::WriteBack,
    )
    .unwrap();
    // RWX, type 6 in bits 5:3, ignore PAT, large, suppress #VE.
    assert_eq!(
        ram.0,
        0x0040_0000 | 0b111 | 6 << 3 | IGNORE_PAT | LARGE | SUPPRESS_VE
    );
    assert_eq!(ram.check(2, 52), Ok(Kind::Leaf(Size::Page2M)));
    assert_eq!(ram.memory_type(), Ok(MemoryType::WriteBack));
    let rom = EptEntry::leaf(
        0x000F_0000,
        Size::Page4K,
        Access::READ,
        MemoryType::Uncacheable,
    )
    .unwrap();
    assert_eq!(rom.0 & 0b111, READ);
    assert!(rom.readable() && !rom.writable() && !rom.executable() && !rom.large());
    assert_eq!(rom.check(1, 52), Ok(Kind::Leaf(Size::Page4K)));
    let gig = EptEntry::leaf(
        0x4000_0000,
        Size::Page1G,
        Access::RAM,
        MemoryType::WriteBack,
    )
    .unwrap();
    assert_eq!(
        (gig.address(), gig.check(3, 52)),
        (0x4000_0000, Ok(Kind::Leaf(Size::Page1G)))
    );
    assert_eq!(
        EptEntry::leaf(0x1000, Size::Page2M, Access::RAM, MemoryType::WriteBack),
        Err(BadEntry::Address(0x1000))
    );
}

#[test]
fn rights_that_are_misconfigurations_are_refused() {
    let leaf = |access| EptEntry::leaf(0x1000, Size::Page4K, access, MemoryType::WriteBack);
    let w = Access {
        read: false,
        write: true,
        execute: false,
    };
    let x = Access {
        read: false,
        write: false,
        execute: true,
    };
    let none = Access {
        read: false,
        write: false,
        execute: false,
    };
    assert_eq!(leaf(w), Err(BadEntry::WriteWithoutRead));
    assert_eq!(leaf(x), Err(BadEntry::ExecuteOnly));
    assert_eq!(leaf(none), Err(BadEntry::NoAccess));
    // The same, read from a table.
    assert_eq!(
        EptEntry(0x1000 | WRITE).check(1, 52),
        Err(BadEntry::WriteWithoutRead)
    );
    assert_eq!(
        EptEntry(0x1000 | EXECUTE).check(1, 52),
        Err(BadEntry::ExecuteOnly)
    );
    assert_eq!(
        EptEntry(0x1000 | USER_EXECUTE).check(1, 52),
        Err(BadEntry::ExecuteOnly)
    );
}

#[test]
fn a_read_entry_is_checked_for_its_level() {
    // No right: not present, whatever else is set.
    assert_eq!(
        EptEntry(0xFFF0_0000_0000_0000 | ACCESSED).check(1, 52),
        Ok(Kind::NotPresent)
    );
    // Reserved memory types 2, 3 and 7.
    for t in [2u64, 3, 7] {
        assert_eq!(
            EptEntry(0x1000 | READ | t << 3).check(1, 52),
            Err(BadEntry::MemoryType(t as u8))
        );
    }
    // A PML4E cannot be large; a table entry's bits 7:3 are reserved.
    assert_eq!(
        EptEntry(0x1000 | READ | LARGE).check(4, 52),
        Err(BadEntry::LargeAtTop(0x1000 | READ | LARGE))
    );
    assert_eq!(
        EptEntry(0x1000 | READ | 6 << 3).check(3, 52),
        Err(BadEntry::Reserved(0x1000 | READ | 6 << 3))
    );
    // A 2 MiB leaf's bits 20:12 are reserved.
    let low = 0x0020_1000 | READ | LARGE | 6 << 3;
    assert_eq!(EptEntry(low).check(2, 52), Err(BadEntry::Reserved(low)));
    // Address bits past MAXPHYADDR are reserved: 46 bits here.
    let high = 1 << 46 | READ | 6 << 3;
    assert_eq!(EptEntry(high).check(1, 46), Err(BadEntry::Reserved(high)));
    assert_eq!(EptEntry(high).check(1, 47), Ok(Kind::Leaf(Size::Page4K)));
    // Level 0 and 6 do not exist.
    assert_eq!(
        EptEntry(0x1000 | READ).check(0, 52),
        Err(BadEntry::Reserved(0x1000 | READ))
    );
    assert_eq!(
        EptEntry(0x1000 | READ).check(6, 52),
        Err(BadEntry::Reserved(0x1000 | READ))
    );
}

/// IA32_VMX_EPT_VPID_CAP as the census read it (g16-vmx-census.md): the Xeon
/// 6975P-C's, and the 8573C's, which has no A/D flags (bit 21). Neither has
/// the uncacheable walk type (bit 8): WB (bit 14) and 4- and 5-level walks.
const CAP_6975: u64 = 0x0F01_0633_40C1;
const CAP_8573: u64 = 0x0F01_0613_40C0;

#[test]
fn the_ept_pointer() {
    let all = u64::MAX;
    let eptp = Eptp::new(0x0010_0000).unwrap();
    // WB (6), walk length 4 - 1 = 3 in bits 5:3.
    assert_eq!(eptp.encode(), Ok(0x0010_0000 | 3 << 3 | 6));
    assert_eq!(Eptp::decode(eptp.encode().unwrap(), 52, all), Ok(eptp));
    let ad = Eptp {
        accessed_dirty: true,
        levels: 5,
        memory: MemoryType::Uncacheable,
        ..eptp
    };
    assert_eq!(ad.encode(), Ok(0x0010_0000 | 1 << 6 | 4 << 3));
    assert_eq!(Eptp::decode(ad.encode().unwrap(), 52, all), Ok(ad));
    assert_eq!(Eptp::new(0x0010_0800), Err(BadEntry::Address(0x0010_0800)));
    // Only UC and WB; only 4 or 5 levels; bits 11:8 and 63:52 reserved.
    assert_eq!(
        Eptp::decode(0x1000 | 3 << 3 | 4, 52, all),
        Err(BadEntry::MemoryType(4))
    );
    assert_eq!(
        Eptp::decode(0x1000 | 2 << 3 | 6, 52, all),
        Err(BadEntry::WalkLength(0x1000 | 2 << 3 | 6))
    );
    assert_eq!(
        Eptp::decode(0x1100 | 3 << 3 | 6, 52, all),
        Err(BadEntry::Reserved(0x1100 | 3 << 3 | 6))
    );
    assert_eq!(
        Eptp::decode(1 << 63 | 3 << 3 | 6, 52, all),
        Err(BadEntry::Reserved(1 << 63 | 3 << 3 | 6))
    );
}

/// #1492's review: `encode` underflowed at 0 levels and wrote a walk length
/// VM entry refuses for 6 or more; and a memory type a pointer cannot have.
#[test]
fn an_ept_pointer_with_a_walk_length_entry_refuses_is_not_encoded() {
    let eptp = Eptp::new(0x1000).unwrap();
    for levels in [0, 1, 2, 3, 6, 7, 8, 9, 255] {
        assert_eq!(
            Eptp { levels, ..eptp }.encode(),
            Err(BadEntry::WalkLength(levels as u64)),
            "{levels} levels"
        );
    }
    for levels in [4, 5] {
        assert!(Eptp { levels, ..eptp }.encode().is_ok());
    }
    assert_eq!(
        Eptp {
            memory: MemoryType::WriteThrough,
            ..eptp
        }
        .encode(),
        Err(BadEntry::MemoryType(4))
    );
}

/// #1492's review: `decode` checked neither the address against the CPU's
/// MAXPHYADDR nor the pointer against EPT_VPID_CAP.
#[test]
fn an_ept_pointer_is_checked_against_the_cpu() {
    let wb4 = 0x1000 | 3 << 3 | 6;
    // Address bits 51:MAXPHYADDR must be clear: bit 39 of a 39-bit CPU.
    let high = 1u64 << 39 | wb4;
    assert_eq!(
        Eptp::decode(high, 39, u64::MAX),
        Err(BadEntry::Reserved(high))
    );
    assert_eq!(
        Eptp::decode(high, 40, u64::MAX).unwrap().root,
        1 << 39 | 0x1000
    );
    let top = 1u64 << 51 | wb4;
    assert!(Eptp::decode(top, 52, u64::MAX).is_ok());
    assert_eq!(
        Eptp::decode(top, 51, u64::MAX),
        Err(BadEntry::Reserved(top))
    );
    // What the CPU's capability MSR lacks, one bit at a time.
    let unsupported =
        |raw: u64, cap: u64| Eptp::decode(raw, 52, cap) == Err(BadEntry::Unsupported(raw));
    let all = u64::MAX;
    let uc4 = 0x1000 | 3 << 3;
    let wb5 = 0x1000 | 4 << 3 | 6;
    let ad = wb4 | 1 << 6;
    let shadow = wb4 | 1 << 7;
    assert!(Eptp::decode(wb4, 52, all).is_ok());
    assert!(unsupported(wb4, all & !cap::WALK_4));
    assert!(unsupported(wb4, all & !cap::WRITE_BACK));
    assert!(unsupported(uc4, all & !cap::UNCACHEABLE));
    assert!(unsupported(wb5, all & !cap::WALK_5));
    assert!(unsupported(ad, all & !cap::ACCESSED_DIRTY));
    assert!(unsupported(shadow, all & !cap::SHADOW_STACK));
    // The accessed and dirty flags are unused unless asked for, and bit 7
    // is not kept.
    assert!(Eptp::decode(wb4, 52, all & !cap::ACCESSED_DIRTY).is_ok());
    assert_eq!(Eptp::decode(shadow, 52, all), Eptp::decode(wb4, 52, all));
    // The two Intel models of the census: write-back, 4 and 5 levels on
    // both, A/D flags on the 6975P-C alone, and uncacheable on neither.
    for cap in [CAP_6975, CAP_8573] {
        assert!(Eptp::decode(wb4, 46, cap).is_ok());
        assert!(Eptp::decode(wb5, 46, cap).is_ok());
        assert!(unsupported(uc4, cap));
    }
    assert!(Eptp::decode(ad, 46, CAP_6975).is_ok());
    assert!(unsupported(ad, CAP_8573));
}

#[test]
fn a_walk_takes_nine_bits_a_level() {
    assert_eq!(indices(0xFEE0_0000), [0, 3, 0x1F7, 0]);
    assert_eq!(indices(0x0000_FFFF_FFFF_F000), [511, 511, 511, 511]);
    assert_eq!((size_of::<Table>(), align_of::<Table>()), (4096, 4096));
    assert_eq!(ADDRESS, 0x000F_FFFF_FFFF_F000);
}
