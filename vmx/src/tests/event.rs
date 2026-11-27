//! The interruption-information format against vol. 3C 25.8.3 (table 25-17)
//! and the injection checks of 26.2.1.3.

use crate::event::{Event, EventType, ExitEvent, Refusal};

#[test]
fn injections_encode_as_table_25_17_gives() {
    assert_eq!(Event::interrupt(0x20).encode(), 0x8000_0020);
    assert_eq!(Event::nmi().encode(), 0x8000_0202);
    // #GP(0): type 3, error code delivered.
    assert_eq!(Event::exception(13, Some(0)).unwrap().encode(), 0x8000_0B0D);
    // #UD: no error code.
    assert_eq!(Event::exception(6, None).unwrap().encode(), 0x8000_0306);
    let int3 = Event {
        vector: 3,
        kind: EventType::SoftwareException,
        error_code: None,
        length: 1,
    };
    assert_eq!((int3.check(), int3.encode()), (Ok(()), 0x8000_0603));
}

#[test]
fn injections_entry_would_refuse_are_refused() {
    assert_eq!(Event::exception(32, None), Err(Refusal::Vector(32)));
    // #PF needs an error code; #UD has none.
    assert_eq!(Event::exception(14, None), Err(Refusal::ErrorCode(14)));
    assert_eq!(Event::exception(6, Some(0)), Err(Refusal::ErrorCode(6)));
    let nmi = Event {
        vector: 3,
        ..Event::nmi()
    };
    assert_eq!(nmi.check(), Err(Refusal::Vector(3)));
    let other = Event {
        vector: 1,
        kind: EventType::Other,
        error_code: None,
        length: 0,
    };
    assert_eq!(other.check(), Err(Refusal::Vector(1)));
    // INTn needs its instruction length, 1-15.
    let int = Event {
        vector: 0x80,
        kind: EventType::SoftwareInterrupt,
        error_code: None,
        length: 0,
    };
    assert_eq!(int.check(), Err(Refusal::Length(0)));
    assert_eq!(
        Event { length: 16, ..int }.check(),
        Err(Refusal::Length(16))
    );
    assert_eq!(Event { length: 2, ..int }.check(), Ok(()));
    // An interrupt with an error code is refused.
    let coded = Event {
        error_code: Some(1),
        ..Event::interrupt(0x30)
    };
    assert_eq!(coded.check(), Err(Refusal::ErrorCode(0x30)));
}

#[test]
fn exit_information_decodes_or_is_refused() {
    assert_eq!(ExitEvent::decode(0), Ok(None));
    // Bits set without the valid bit are no event.
    assert_eq!(ExitEvent::decode(0x0B0E), Ok(None));
    let pf = ExitEvent::decode(0x8000_1B0E).unwrap().unwrap();
    assert_eq!((pf.vector, pf.kind), (14, EventType::HardwareException));
    assert!(pf.has_error_code && pf.nmi_unblocking);
    // Type 1 is reserved, and so are bits 30:13.
    assert_eq!(
        ExitEvent::decode(0x8000_0100),
        Err(Refusal::ReservedType(1))
    );
    assert_eq!(
        ExitEvent::decode(0x8000_2000),
        Err(Refusal::Reserved(0x8000_2000))
    );
    assert_eq!(EventType::from_raw(1), Err(Refusal::ReservedType(1)));
    assert_eq!(EventType::from_raw(8), Err(Refusal::ReservedType(8)));
}

/// #1492's review: with unrestricted guest and CR0.PE clear (real mode), VM
/// entry refuses an error code on #GP and the like (26.2.1.3).
#[test]
fn a_real_mode_guest_takes_exceptions_without_error_codes() {
    // Protected mode, as before.
    assert!(Event::exception_in(13, Some(0), true).is_ok());
    assert_eq!(
        Event::exception_in(13, None, true),
        Err(Refusal::ErrorCode(13))
    );
    // Real mode: #GP, #PF, #DF, #TS, #NP, #SS and #AC carry none.
    for vector in [8, 10, 11, 12, 13, 14, 17, 21] {
        assert_eq!(
            Event::exception_in(vector, Some(0), false),
            Err(Refusal::ErrorCode(vector)),
            "#{vector}"
        );
        let event = Event::exception_in(vector, None, false).unwrap();
        assert_eq!(event.encode() & 1 << 11, 0, "no error-code bit");
        assert_eq!(event.check_in(false), Ok(()));
        // The same event is refused for a protected-mode guest.
        assert_eq!(event.check(), Err(Refusal::ErrorCode(vector)));
    }
    // Vectors that deliver none are the same in both modes.
    assert!(Event::exception_in(6, None, false).is_ok());
    assert!(Event::exception_in(6, Some(0), false).is_err());
    // The mode does not touch other event types: an interrupt, an NMI and an
    // INTn are checked as before.
    assert_eq!(Event::interrupt(0x20).check_in(false), Ok(()));
    assert_eq!(Event::nmi().check_in(false), Ok(()));
    let coded = Event {
        error_code: Some(1),
        ..Event::interrupt(0x30)
    };
    assert_eq!(coded.check_in(false), Err(Refusal::ErrorCode(0x30)));
    assert_eq!(
        Event::exception_in(32, None, false),
        Err(Refusal::Vector(32))
    );
}
