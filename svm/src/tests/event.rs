//! EVENTINJ's bits from APM vol. 2, 15.20: vector 7:0, type 10:8, EV 11,
//! V 31, the error code in 63:32.

use crate::event::{Event, EventType, Refusal, pushes_error_code};
use crate::vmcb::Vmcb;

#[test]
fn events_encode_as_the_apm_lays_them_out() {
    // An external interrupt, vector 0x20: V and the vector.
    assert_eq!(Event::interrupt(0x20).encode(), 0x8000_0020);
    // An NMI: type 2.
    assert_eq!(Event::nmi().encode(), 0x8000_0202);
    // #GP(0x18): type 3, EV, the error code above.
    let gp = Event::exception(13, Some(0x18)).unwrap();
    assert_eq!(gp.encode(), 0x0000_0018_8000_0B0D);
    // #UD: no error code.
    assert_eq!(Event::exception(6, None).unwrap().encode(), 0x8000_0306);
    // INT 0x80: type 4.
    assert_eq!(Event::software(0x80).encode(), 0x8000_0480);
}

#[test]
fn an_exception_must_carry_exactly_the_error_code_it_pushes() {
    assert_eq!(Event::exception(14, None), Err(Refusal::ErrorCode(14)));
    assert_eq!(Event::exception(6, Some(0)), Err(Refusal::ErrorCode(6)));
    assert_eq!(Event::exception(32, None), Err(Refusal::NotAnException(32)));
    // The NMI has its own type, and 9, 15, 22-27 and 31 are reserved.
    for vector in [2, 9, 15, 22, 23, 24, 25, 26, 27, 31] {
        assert_eq!(
            Event::exception(vector, None),
            Err(Refusal::ReservedVector(vector))
        );
    }
    // 28-30 (#HV, #VC, #SX) are exceptions.
    assert!(Event::exception(28, None).is_ok());
    assert!(Event::exception(30, Some(0)).is_ok());
    for vector in 0..32u8 {
        let pushes = matches!(vector, 8 | 10..=14 | 17 | 21 | 29 | 30);
        assert_eq!(pushes_error_code(vector), pushes, "vector {vector}");
    }
}

#[test]
fn an_event_reads_back_and_an_invalid_field_is_none() {
    let events = [
        Event::interrupt(0xEC),
        Event::nmi(),
        Event::exception(14, Some(0b101)).unwrap(),
        Event::software(3),
    ];
    for event in events {
        assert_eq!(Event::decode(event.encode()), Ok(Some(event)));
    }
    assert_eq!(Event::decode(0x0000_0020), Ok(None));
    assert_eq!(Event::decode(0x8000_0120), Err(Refusal::ReservedType(1)));
    assert_eq!(Event::decode(0x8000_0720), Err(Refusal::ReservedType(7)));
}

#[test]
fn an_injected_event_lands_in_eventinj_and_exitintinfo_reads_one_back() {
    let mut vmcb = Vmcb::zeroed();
    let pf = Event::exception(14, Some(2)).unwrap();
    pf.inject(&mut vmcb.control);
    assert_eq!(vmcb.control.event_inj, 0x8000_0B0E);
    assert_eq!(vmcb.control.event_inj_error, 2);
    assert_eq!(Event::interrupted(&vmcb.control), Ok(None));
    // The CPU reports the #PF it was delivering when the exit came.
    vmcb.control.exit_int_info = vmcb.control.event_inj;
    vmcb.control.exit_int_info_error = vmcb.control.event_inj_error;
    let again = Event::interrupted(&vmcb.control).unwrap().unwrap();
    assert_eq!(again, pf);
    assert_eq!(again.kind, EventType::Exception);
}
