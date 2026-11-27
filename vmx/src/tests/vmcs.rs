//! IA32_VMX_BASIC against vol. 3D A.1, the region header of vol. 3C 25.2,
//! and the VM-instruction errors of 31.4 (table 31-1).

use crate::vmcs::{BadBasic, Basic, InstructionError, Region};
use core::mem::{align_of, size_of};

#[test]
fn basic_decodes_a_typical_cpus_value() {
    // Revision 4, a 4 KiB region, write-back, INS/OUTS info, TRUE_ MSRs.
    let raw = 4 | 0x1000 << 32 | 6 << 50 | 1 << 54 | 1 << 55;
    let basic = Basic::decode(raw).unwrap();
    assert_eq!((basic.revision, basic.region_size), (4, 4096));
    assert!(
        basic.write_back && basic.ins_outs_info && basic.true_controls && !basic.addresses_32bit
    );
    let region = Region::new(&basic);
    assert_eq!(
        (region.revision, region.abort, region.shadow()),
        (4, 0, false)
    );
    assert_eq!((size_of::<Region>(), align_of::<Region>()), (4096, 4096));
}

#[test]
fn a_bad_basic_is_refused() {
    let ok = 4 | 0x1000 << 32 | 6 << 50;
    assert_eq!(
        Basic::decode(ok | 1 << 31),
        Err(BadBasic::Bit31(ok | 1 << 31))
    );
    assert_eq!(Basic::decode(4 | 6 << 50), Err(BadBasic::RegionSize(0)));
    assert_eq!(
        Basic::decode(4 | 0x1001 << 32 | 6 << 50),
        Err(BadBasic::RegionSize(0x1001))
    );
    assert_eq!(
        Basic::decode(4 | 0x1000 << 32 | 5 << 50),
        Err(BadBasic::MemoryType(5))
    );
    assert_eq!(
        Basic::decode(4 | 0x1000 << 32).map(|b| b.write_back),
        Ok(false)
    );
}

#[test]
fn instruction_errors_have_table_31_1s_numbers() {
    assert_eq!(
        InstructionError::from_raw(7),
        Ok(InstructionError::EntryInvalidControls)
    );
    assert_eq!(
        InstructionError::from_raw(8),
        Ok(InstructionError::EntryInvalidHostState)
    );
    assert_eq!(
        InstructionError::from_raw(12),
        Ok(InstructionError::UnsupportedField)
    );
    assert_eq!(
        InstructionError::from_raw(26),
        Ok(InstructionError::EntryBlockedByMovSs)
    );
    assert_eq!(
        InstructionError::from_raw(28),
        Ok(InstructionError::InvalidInvOperand)
    );
    for raw in [0, 14, 21, 27, 29, u32::MAX] {
        assert_eq!(InstructionError::from_raw(raw), Err(raw));
    }
    for raw in 1..=28 {
        if let Ok(e) = InstructionError::from_raw(raw) {
            assert_eq!(e as u32, raw);
        }
    }
}
