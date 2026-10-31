//! The ledger of the Settings app's requests: refusals, the host's own
//! tickets, room at `desktop`, and answers that go only to the instance that
//! asked.

use crate::settings::{Kind, SettingsRequest};
use crate::settings_ledger::{Ledger, NOT_BY_USER, Owed};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

fn request(ticket: u32, text: &str, kind: Kind, user: bool) -> SettingsRequest {
    SettingsRequest {
        ticket,
        request: String::from(text),
        kind,
        user,
    }
}

fn read(ticket: u32) -> SettingsRequest {
    request(ticket, "rules", Kind::Read, false)
}

#[test]
fn a_change_without_a_key_or_click_is_refused_and_never_leaves() {
    let mut ledger = Ledger::default();
    let refused = ledger.queue(
        7,
        vec![
            request(0, "grants-revoke 3", Kind::Change, false),
            request(1, "grants-revoke 3", Kind::Change, true),
            read(2),
        ],
    );
    assert_eq!(
        refused,
        vec![Owed {
            serial: 7,
            ticket: 0,
            outcome: Err(String::from(NOT_BY_USER)),
        }]
    );
    let out = ledger.outgoing(8);
    let texts: Vec<_> = out.iter().map(|o| (o.text.as_str(), o.user)).collect();
    assert_eq!(texts, [("grants-revoke 3", true), ("rules", false)]);
}

#[test]
fn the_wire_ticket_is_the_hosts_own_and_answers_find_their_asker() {
    let mut ledger = Ledger::default();
    // Two instances both used ticket 0: the wire keeps them apart.
    ledger.queue(1, vec![read(0)]);
    ledger.queue(2, vec![read(0)]);
    let out = ledger.outgoing(8);
    assert_eq!(out.len(), 2);
    assert_ne!(out[0].wire, out[1].wire);
    let second = ledger.answered(out[1].wire, Ok(b"two".to_vec())).unwrap();
    assert_eq!((second.serial, second.ticket), (2, 0));
    let first = ledger.answered(out[0].wire, Ok(b"one".to_vec())).unwrap();
    assert_eq!((first.serial, first.ticket), (1, 0));
    // Answered once only, and a ticket it never sent finds no one.
    assert_eq!(ledger.answered(out[0].wire, Ok(Vec::new())), None);
    assert_eq!(ledger.answered(999, Ok(Vec::new())), None);
    assert!(!ledger.busy());
}

#[test]
fn only_as_many_leave_as_there_is_room_and_the_rest_wait() {
    let mut ledger = Ledger::default();
    ledger.queue(1, (0..5).map(read).collect());
    assert_eq!(ledger.outgoing(3).len(), 3);
    assert_eq!(ledger.in_flight(), 3);
    assert!(ledger.busy());
    let rest = ledger.outgoing(8);
    assert_eq!(rest.len(), 2);
    assert_eq!(ledger.outgoing(8), vec![]);
}

#[test]
fn requests_that_never_arrived_are_owed_why() {
    let mut ledger = Ledger::default();
    ledger.queue(4, vec![read(5), read(6)]);
    let wires: Vec<u32> = ledger.outgoing(8).iter().map(|o| o.wire).collect();
    let owed = ledger.refused(&wires, "desktop could not be asked");
    let tickets: Vec<_> = owed.iter().map(|o| (o.ticket, o.outcome.clone())).collect();
    assert_eq!(
        tickets,
        [
            (5, Err(String::from("desktop could not be asked"))),
            (6, Err(String::from("desktop could not be asked"))),
        ]
    );
    assert!(!ledger.busy());
}

#[test]
fn an_answer_goes_only_to_the_instance_still_shown() {
    let owed = Owed {
        serial: 3,
        ticket: 0,
        outcome: Ok(Vec::new()),
    };
    assert!(owed.is_for(Some(3)));
    // Reloaded, swapped or resumed: a new serial, which never asked.
    assert!(!owed.is_for(Some(4)));
    // Suspended: no instance, and the answer must not wake one.
    assert!(!owed.is_for(None));
}

#[test]
fn every_load_has_a_serial_of_its_own() {
    let limits = crate::Limits {
        fuel_call: 1_000_000,
        fuel_life: 1_000_000,
        memory_bytes: 1 << 20,
    };
    let module = wat::parse_str(r#"(module (memory (export "memory") 1))"#).unwrap();
    let load = || crate::load_core(&module, limits, None).unwrap().serial();
    let (a, b) = (load(), load());
    assert_ne!(a, b);
}
