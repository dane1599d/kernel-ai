//! `kai:net`: the grant that links it, the queue `fetch` fills, and the
//! answers `on-response` takes, through a hand-written canonical-ABI guest.

extern crate std;

use crate::net::{MAX_BODY, MAX_QUEUED, NetError, NetRequest, NetResponse};
use crate::{CoreInstance, Grants, Limits, load_core, load_core_granted};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use livecore::{Fault, Instance, Reason};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

const URL: &str = "https://updates.kai.test/hello";

/// A guest whose `health` makes `fetches` requests for [`URL`] (GET, the
/// first with `range=2-`, the rest whole; HEAD when `head`) and returns the
/// last ticket, and whose `on-response` keeps the ticket, the result's case
/// and its first flat value (a response's status, an error's case), which
/// `self-test` reads back as one number. `trap` ends `health` in an
/// unreachable after the requests.
fn guest_with(fetches: usize, head: bool, trap: bool) -> Vec<u8> {
    let method = u8::from(head);
    let mut body = String::new();
    for i in 0..fetches {
        let (range, first) = if i == 0 { (1, 2) } else { (0, 0) };
        body.push_str(&format!(
            "i32.const 1024\ni32.const {}\ni32.const {method}\ni32.const {range}\ni64.const {first}\ni32.const 0\ni64.const 0\ncall $fetch\n",
            URL.len()
        ));
        if i + 1 < fetches {
            body.push_str("drop\n");
        }
    }
    if fetches == 0 {
        body.push_str("i32.const 0\n");
    }
    let trap = if trap { "unreachable" } else { "" };
    let source = format!(
        r#"(module
  (import "kai:net/http@0.1.0" "fetch"
    (func $fetch (param i32 i32 i32 i32 i64 i32 i64) (result i32)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "{URL}")
  (global $top (mut i32) (i32.const 40000))
  (global $ticket (mut i32) (i32.const 0))
  (global $case (mut i32) (i32.const 0))
  (global $first (mut i32) (i32.const 0))
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (local $at i32)
    global.get $top
    local.set $at
    global.get $top
    local.get 3
    i32.add
    global.set $top
    local.get $at)
  (func (export "kai:live/lifecycle@0.1.0#health") (result i32)
    {body}
    {trap})
  (func (export "kai:live/lifecycle@0.1.0#self-test") (result i32)
    global.get $ticket
    i32.const 24
    i32.shl
    global.get $case
    i32.const 16
    i32.shl
    i32.or
    global.get $first
    i32.or)
  (func (export "kai:net/responses@0.1.0#on-response")
    (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
    local.get 0
    global.set $ticket
    local.get 1
    global.set $case
    local.get 2
    global.set $first))"#
    );
    wat::parse_str(source).unwrap()
}

fn granted() -> Grants {
    Grants {
        net: true,
        ..Grants::default()
    }
}

fn app_fetching(fetches: usize) -> CoreInstance {
    load_core_granted(&guest_with(fetches, false, false), LIMITS, None, granted()).unwrap()
}

/// What `on-response` kept: the ticket, the result's case, the first value.
fn kept(app: &mut CoreInstance) -> (u32, u32, u32) {
    let word = app.self_test().unwrap();
    (word >> 24, (word >> 16) & 0xff, word & 0xffff)
}

fn response(status: u16, body: Vec<u8>) -> NetResponse {
    NetResponse {
        status,
        url: URL.to_string(),
        content_type: Some(String::from("text/plain")),
        retry_after: None,
        body,
    }
}

#[test]
fn an_app_without_the_grant_does_not_link() {
    let guest = guest_with(1, false, false);
    let fault = match load_core(&guest, LIMITS, None) {
        Err(fault) => fault,
        Ok(_) => panic!("linked without the grant"),
    };
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("net grant"), "{}", fault.detail);
    let models = Grants {
        models: true,
        ..Grants::default()
    };
    assert!(load_core_granted(&guest, LIMITS, None, models).is_err());
    assert!(load_core_granted(&guest, LIMITS, None, granted()).is_ok());
}

#[test]
fn fetch_queues_the_request_and_the_host_takes_it() {
    let mut app = app_fetching(2);
    assert!(!app.net_waiting());
    assert_eq!(app.health().unwrap(), 1, "the second ticket");
    assert!(app.net_waiting());
    let taken = app.take_net();
    let first = NetRequest {
        url: URL.to_string(),
        head: false,
        range: Some((2, None)),
    };
    let whole = NetRequest {
        range: None,
        ..first.clone()
    };
    assert_eq!(taken, vec![(0, first), (1, whole)]);
    assert!(!app.net_waiting());
    assert!(app.take_net().is_empty());
}

#[test]
fn a_head_request_says_so() {
    let mut app = load_core_granted(&guest_with(1, true, false), LIMITS, None, granted()).unwrap();
    app.health().unwrap();
    assert!(app.take_net()[0].1.head);
}

#[test]
fn more_than_the_queue_holds_traps_and_a_trap_queues_nothing() {
    let mut app = app_fetching(MAX_QUEUED + 1);
    assert!(app.health().is_err());
    assert!(!app.net_waiting(), "the faulted call queued nothing");
    let mut app = load_core_granted(&guest_with(1, false, true), LIMITS, None, granted()).unwrap();
    assert!(app.health().is_err());
    assert!(!app.net_waiting());
}

#[test]
fn the_app_is_told_a_response_or_an_error() {
    let mut app = app_fetching(1);
    app.health().unwrap();
    let (ticket, _) = app.take_net().remove(0);
    app.net_response(ticket, Ok(response(200, b"kai mock: hello".to_vec())))
        .unwrap();
    assert_eq!(kept(&mut app), (0, 0, 200));
    app.net_response(
        7,
        Err(NetError::Refused(String::from("elsewhere.kai.test"))),
    )
    .unwrap();
    assert_eq!(kept(&mut app), (7, 1, 0), "refused is the first case");
    app.net_response(8, Err(NetError::Timeout)).unwrap();
    assert_eq!(kept(&mut app), (8, 1, 5));
}

#[test]
fn a_body_over_the_bound_is_too_large() {
    let mut app = app_fetching(0);
    app.net_response(3, Ok(response(200, vec![0; MAX_BODY + 1])))
        .unwrap();
    assert_eq!(kept(&mut app), (3, 1, 4), "too-large");
    app.net_response(4, Ok(response(206, vec![0; MAX_BODY])))
        .unwrap();
    assert_eq!(kept(&mut app), (4, 0, 206));
}

#[test]
fn an_app_without_the_export_is_told_nothing() {
    let source = r#"(module
  (import "kai:net/http@0.1.0" "fetch"
    (func $fetch (param i32 i32 i32 i32 i64 i32 i64) (result i32)))
  (memory (export "memory") 1)
  (func (export "kai:live/lifecycle@0.1.0#health") (result i32) i32.const 0))"#;
    let guest = wat::parse_str(source).unwrap();
    let mut app: CoreInstance = load_core_granted(&guest, LIMITS, None, granted()).unwrap();
    let told: Result<(), Fault> = app.net_response(0, Err(NetError::Timeout));
    assert!(told.is_ok());
}
