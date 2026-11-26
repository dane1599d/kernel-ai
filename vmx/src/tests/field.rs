//! Field encodings against vol. 3D appendix B: each named field is the
//! encoding its width, type and index give (B.1-B.4), and the format refuses
//! what VMREAD would.

use crate::field::{ALL, BadField, Field, Kind, Width};

#[test]
fn named_fields_are_their_width_type_and_index() {
    // A row from each of appendix B's twelve tables.
    let table: &[(Field, Width, Kind, u16)] = &[
        (Field::VPID, Width::Bits16, Kind::Control, 0),
        (Field::GUEST_TR_SELECTOR, Width::Bits16, Kind::Guest, 7),
        (Field::HOST_TR_SELECTOR, Width::Bits16, Kind::Host, 6),
        (Field::EPT_POINTER, Width::Bits64, Kind::Control, 13),
        (
            Field::GUEST_PHYSICAL_ADDRESS,
            Width::Bits64,
            Kind::ExitInfo,
            0,
        ),
        (Field::VMCS_LINK_POINTER, Width::Bits64, Kind::Guest, 0),
        (Field::HOST_IA32_EFER, Width::Bits64, Kind::Host, 1),
        (
            Field::SECONDARY_PROCBASED_CONTROLS,
            Width::Bits32,
            Kind::Control,
            15,
        ),
        (Field::EXIT_REASON, Width::Bits32, Kind::ExitInfo, 1),
        (
            Field::GUEST_INTERRUPTIBILITY,
            Width::Bits32,
            Kind::Guest,
            18,
        ),
        (Field::HOST_IA32_SYSENTER_CS, Width::Bits32, Kind::Host, 0),
        (Field::CR4_READ_SHADOW, Width::Natural, Kind::Control, 3),
        (Field::EXIT_QUALIFICATION, Width::Natural, Kind::ExitInfo, 0),
        (Field::GUEST_RIP, Width::Natural, Kind::Guest, 15),
        (Field::HOST_RIP, Width::Natural, Kind::Host, 11),
    ];
    for &(field, width, kind, index) in table {
        assert_eq!(Field::new(width, kind, index), field, "{field:?}");
        assert_eq!(
            (field.width(), field.kind(), field.index()),
            (width, kind, index)
        );
    }
}

#[test]
fn appendix_bs_values() {
    let table: &[(Field, u32)] = &[
        (Field::VPID, 0x0000),
        (Field::GUEST_ES_SELECTOR, 0x0800),
        (Field::HOST_CS_SELECTOR, 0x0C02),
        (Field::IO_BITMAP_A, 0x2000),
        (Field::MSR_BITMAP, 0x2004),
        (Field::EPT_POINTER, 0x201A),
        (Field::GUEST_IA32_EFER, 0x2806),
        (Field::PINBASED_CONTROLS, 0x4000),
        (Field::PROCBASED_CONTROLS, 0x4002),
        (Field::EXCEPTION_BITMAP, 0x4004),
        (Field::EXIT_CONTROLS, 0x400C),
        (Field::ENTRY_CONTROLS, 0x4012),
        (Field::ENTRY_INTERRUPTION_INFO, 0x4016),
        (Field::SECONDARY_PROCBASED_CONTROLS, 0x401E),
        (Field::VM_INSTRUCTION_ERROR, 0x4400),
        (Field::EXIT_REASON, 0x4402),
        (Field::EXIT_INTERRUPTION_INFO, 0x4404),
        (Field::IDT_VECTORING_INFO, 0x4408),
        (Field::EXIT_INSTRUCTION_LENGTH, 0x440C),
        (Field::GUEST_CS_ACCESS_RIGHTS, 0x4816),
        (Field::GUEST_ACTIVITY_STATE, 0x4826),
        (Field::EXIT_QUALIFICATION, 0x6400),
        (Field::GUEST_LINEAR_ADDRESS, 0x640A),
        (Field::GUEST_CR0, 0x6800),
        (Field::GUEST_CR3, 0x6802),
        (Field::GUEST_RSP, 0x681C),
        (Field::GUEST_RIP, 0x681E),
        (Field::GUEST_RFLAGS, 0x6820),
        (Field::HOST_CR3, 0x6C02),
        (Field::HOST_RSP, 0x6C14),
        (Field::HOST_RIP, 0x6C16),
    ];
    for &(field, raw) in table {
        assert_eq!(field.raw(), raw, "{field:?}");
    }
}

#[test]
fn every_named_field_is_valid_full_and_unique() {
    for (i, &(field, name)) in ALL.iter().enumerate() {
        assert_eq!(Field::decode(field.raw()), Ok(field), "{name}");
        assert!(!field.is_high(), "{name}");
        // The names say their section: GUEST_ and HOST_ fields are guest and
        // host state, the read-only data is exit information.
        if name.starts_with("GUEST_")
            && name != "GUEST_PHYSICAL_ADDRESS"
            && name != "GUEST_LINEAR_ADDRESS"
        {
            assert_eq!(field.kind(), Kind::Guest, "{name}");
        }
        if name.starts_with("HOST_") {
            assert_eq!(field.kind(), Kind::Host, "{name}");
        }
        for &(other, other_name) in &ALL[i + 1..] {
            assert_ne!(field, other, "{name} and {other_name}");
        }
    }
    assert!(Field::EXIT_REASON.read_only() && !Field::GUEST_RIP.read_only());
}

#[test]
fn only_a_64_bit_field_has_a_high_half() {
    let high = Field::EPT_POINTER.high().unwrap();
    assert_eq!(high.raw(), 0x201B);
    assert_eq!((high.bytes(), Field::EPT_POINTER.bytes()), (4, 8));
    assert_eq!(high.high(), None);
    assert_eq!(Field::GUEST_RIP.high(), None);
    assert_eq!(Field::EXIT_REASON.high(), None);
    assert_eq!(Field::decode(0x201B), Ok(high));
    assert_eq!(Field::decode(0x4403), Err(BadField::HighOfNarrow(0x4403)));
    assert_eq!(Field::decode(0x681F), Err(BadField::HighOfNarrow(0x681F)));
    assert_eq!(Field::decode(0x0001), Err(BadField::HighOfNarrow(0x0001)));
    assert_eq!(
        (
            Field::VPID.bytes(),
            Field::EXIT_REASON.bytes(),
            Field::GUEST_RIP.bytes()
        ),
        (2, 4, 8)
    );
}

#[test]
fn reserved_bits_are_refused() {
    assert_eq!(Field::decode(0x1000), Err(BadField::Reserved(0x1000)));
    assert_eq!(Field::decode(0x8000), Err(BadField::Reserved(0x8000)));
    assert_eq!(
        Field::decode(0x6800 | 1 << 31),
        Err(BadField::Reserved(0x8000_6800))
    );
    assert_eq!(Field::decode(u32::MAX), Err(BadField::Reserved(u32::MAX)));
    // The index is 9 bits: a larger one is cut, not carried into the type.
    assert_eq!(Field::new(Width::Bits16, Kind::Control, 0x200), Field::VPID);
}
