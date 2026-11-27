//! Every decoder against input the SDM does not define, as a hostile guest
//! or a buggy CPU could leave it: each returns Ok or Err and never panics,
//! and what it accepts reads back as it came.

use super::Noise;
use crate::ept::{EptEntry, Eptp, Kind};
use crate::event::ExitEvent;
use crate::exit::{
    BasicExit, CrAccess, DrAccess, EntryFailure, EptViolation, ExitReason, IoAccess,
};
use crate::field::{Field, Width};
use crate::vmcs::{Basic, InstructionError};

#[test]
fn every_basic_exit_number_and_exit_reason_flag() {
    let mut known = 0;
    for raw in 0..=u16::MAX {
        if let Ok(exit) = BasicExit::from_raw(raw) {
            assert_eq!(exit as u16, raw);
            known += 1;
        }
        // With each flag combination in bits 31:27.
        for flags in 0..32u32 {
            let field = raw as u32 | flags << 27;
            if let Ok(reason) = ExitReason::decode(field) {
                assert_eq!(reason.encode(), field);
            }
        }
    }
    assert_eq!(known, crate::exit::ALL.len());
}

#[test]
fn random_exit_fields_and_qualifications() {
    let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
    for _ in 0..200_000 {
        let x = noise.next();
        if let Ok(reason) = ExitReason::decode(x as u32) {
            assert_eq!(reason.encode(), x as u32);
        }
        if let Ok(io) = IoAccess::decode(x) {
            assert_eq!(io.encode(), x);
        }
        let _ = CrAccess::decode(x);
        let _ = DrAccess::decode(x);
        let _ = EntryFailure::decode(x);
        let _ = EptViolation::decode(x, noise.next());
        let _ = ExitEvent::decode(x as u32);
        let _ = InstructionError::from_raw(x as u32);
        // Low values too, where the formats live.
        let small = x & 0xFFFF_0FFF;
        let _ = CrAccess::decode(small);
        let _ = IoAccess::decode(small);
    }
}

#[test]
fn every_field_encoding_below_bit_16() {
    let mut valid = 0;
    for raw in 0..=0x1_FFFFu32 {
        if let Ok(field) = Field::decode(raw) {
            valid += 1;
            assert_eq!(field.raw(), raw);
            assert!(raw & (1 << 12) == 0 && raw < 0x8000);
            assert!(!field.is_high() || field.width() == Width::Bits64);
            assert!(matches!(field.bytes(), 2 | 4 | 8));
        }
    }
    // 4 types x 512 indices x (3 widths full, plus 64-bit full and high).
    assert_eq!(valid, 4 * 512 * 5);
}

#[test]
fn random_ept_entries_and_pointers() {
    let mut noise = Noise(0x2545_F491_4F6C_DD1D);
    for _ in 0..200_000 {
        let x = noise.next();
        // Mostly-valid entries too: rights and a memory type, random rest.
        let shaped = x & !0x7F | 0b001 | (x >> 61) << 3;
        for raw in [x, shaped] {
            for level in 0..=6 {
                for bits in [36, 46, 52, 60] {
                    if let Ok(Kind::Leaf(_)) = EptEntry(raw).check(level, bits) {
                        assert!(EptEntry(raw).memory_type().is_ok());
                        assert!(EptEntry(raw).readable());
                    }
                }
            }
        }
        // Every capability bit, and a CPU with 40 physical address bits.
        for (bits, cap) in [(52, u64::MAX), (40, u64::MAX), (36, 0)] {
            if let Ok(eptp) = Eptp::decode(x, bits, cap) {
                assert_eq!(eptp.encode(), Ok(x & !(1 << 7)));
                assert_eq!(eptp.root >> bits, 0);
                assert!(cap != 0);
            }
        }
        let _ = Basic::decode(x);
    }
}

#[test]
fn every_interruption_information_value_below_bit_14() {
    for low in 0..(1u32 << 14) {
        for raw in [low, low | 1 << 31, low | 1 << 31 | 1 << 30] {
            match ExitEvent::decode(raw) {
                Ok(Some(event)) => {
                    assert_eq!(event.vector, raw as u8);
                    assert!(raw & 0x7FFF_E000 == 0);
                }
                Ok(None) => assert_eq!(raw & 1 << 31, 0),
                Err(_) => {}
            }
        }
    }
}
