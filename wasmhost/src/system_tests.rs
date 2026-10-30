//! `kai:system`: the grant that links it, the queue `read` fills, and the
//! samples `on-sample` takes, through a hand-written canonical-ABI guest.

extern crate std;

use crate::system::{MAX_QUEUED, MAX_REPLY};
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

/// A guest whose `health` makes `reads` reads (and returns the last ticket, or 0
/// for none), and whose `on-sample` keeps the ticket, the result's case and the
/// payload's length, which `self-test` reads back as one number. `trap` ends
/// `health` in an unreachable after the reads.
fn guest_with(reads: usize, trap: bool, exports_samples: bool) -> Vec<u8> {
    let mut body = String::new();
    for i in 0..reads {
        body.push_str("call $read\n");
        if i + 1 < reads {
            body.push_str("drop\n");
        }
    }
    if reads == 0 {
        body.push_str("i32.const 0\n");
    }
    let trap = if trap { "unreachable" } else { "" };
    let on_sample = if exports_samples {
        r#"(func (export "kai:system/samples@0.1.0#on-sample") (param i32 i32 i32 i32)
    local.get 0
    global.set $ticket
    local.get 1
    global.set $case
    local.get 3
    global.set $size)"#
    } else {
        ""
    };
    let source = format!(
        r#"(module
  (import "kai:system/system@0.1.0" "read" (func $read (result i32)))
  (memory (export "memory") 2)
  (global $top (mut i32) (i32.const 40000))
  (global $ticket (mut i32) (i32.const 0))
  (global $case (mut i32) (i32.const 0))
  (global $size (mut i32) (i32.const 0))
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
    global.get $size
    i32.or)
  {on_sample})"#
    );
    wat::parse_str(source).unwrap()
}

fn granted() -> Grants {
    Grants {
        system: true,
        ..Grants::default()
    }
}

fn app_reading(reads: usize) -> CoreInstance {
    load_core_granted(&guest_with(reads, false, true), LIMITS, None, granted()).unwrap()
}

fn refused(result: Result<CoreInstance, Fault>) -> Fault {
    match result {
        Err(fault) => fault,
        Ok(_) => panic!("expected a fault"),
    }
}

#[test]
fn an_app_without_the_grant_does_not_link() {
    let guest = guest_with(1, false, true);
    let fault = refused(load_core(&guest, LIMITS, None));
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("system grant"), "{}", fault.detail);
    // The settings grant is not the system grant.
    let settings = Grants {
        settings: true,
        ..Grants::default()
    };
    let fault = refused(load_core_granted(&guest, LIMITS, None, settings));
    assert_eq!(fault.reason, Reason::Link);
    assert!(load_core_granted(&guest, LIMITS, None, granted()).is_ok());
}

#[test]
fn the_system_grant_does_not_open_the_settings_interface() {
    let bytes = wat::parse_str(
        r#"(module
  (import "kai:settings/settings@0.1.0" "send" (func (param i32 i32) (result i32)))
  (memory (export "memory") 1))"#,
    )
    .unwrap();
    let fault = refused(load_core_granted(&bytes, LIMITS, None, granted()));
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("settings grant"), "{}", fault.detail);
}

#[test]
fn read_queues_a_ticket_and_taking_empties_the_queue() {
    let mut app = app_reading(1);
    assert_eq!(app.health().unwrap(), 0, "the first ticket");
    assert_eq!(app.health().unwrap(), 1);
    assert_eq!(app.take_system(), vec![0, 1]);
    assert!(app.take_system().is_empty());
    // Tickets go on counting after the queue was taken.
    assert_eq!(app.health().unwrap(), 2);
    assert_eq!(app.take_system(), vec![2]);
}

#[test]
fn a_call_may_make_the_most_reads_and_the_next_one_traps() {
    let mut app = app_reading(MAX_QUEUED);
    assert_eq!(app.health().unwrap(), MAX_QUEUED as u32 - 1);
    // The queue is full: the first read of the next call traps, and what was
    // waiting stays.
    let fault = app.health().expect_err("a full queue refuses another read");
    assert!(fault.detail.contains("waiting"), "{}", fault.detail);
    assert_eq!(
        app.take_system(),
        (0..MAX_QUEUED as u32).collect::<Vec<_>>()
    );
    // Once taken, reads are accepted again, and the tickets go on.
    assert_eq!(app.health().unwrap(), MAX_QUEUED as u32 + 3);
}

#[test]
fn too_many_reads_in_one_call_trap_and_queue_nothing() {
    let mut app = app_reading(MAX_QUEUED + 1);
    app.health().expect_err("the last read of the call traps");
    assert!(app.take_system().is_empty());
}

#[test]
fn a_call_that_faults_leaves_nothing_queued() {
    let mut app = load_core_granted(&guest_with(2, true, true), LIMITS, None, granted()).unwrap();
    app.health().expect_err("it ends in unreachable");
    assert!(app.take_system().is_empty());
}

#[test]
fn a_new_instance_starts_again_from_ticket_zero_with_nothing_waiting() {
    let mut old = app_reading(1);
    old.health().unwrap();
    old.health().unwrap();
    let mut new = app_reading(1);
    assert!(new.take_system().is_empty());
    assert_eq!(new.health().unwrap(), 0);
    assert_eq!(new.take_system(), vec![0]);
    // The old instance's reads were never the new one's.
    assert_eq!(old.take_system(), vec![0, 1]);
}

#[test]
fn a_sample_reaches_the_app_as_a_result_with_its_ticket() {
    let mut app = app_reading(1);
    app.system_sample(7, Ok(b"KSY1".to_vec())).unwrap();
    // The ticket in the top byte, the result's case next (0 is ok), then the
    // payload's length.
    assert_eq!(app.self_test().unwrap(), 7 << 24 | 4);
    app.system_sample(9, Err("the system could not be read".to_string()))
        .unwrap();
    assert_eq!(app.self_test().unwrap(), 9 << 24 | 1 << 16 | 28);
}

#[test]
fn a_big_sample_and_a_long_or_odd_error_are_cut_down_to_what_the_app_can_take() {
    let mut app = app_reading(1);
    app.system_sample(1, Ok(vec![0; MAX_REPLY])).unwrap();
    assert_eq!(app.self_test().unwrap(), 1 << 24 | MAX_REPLY as u32);
    app.system_sample(2, Ok(vec![0; MAX_REPLY + 1])).unwrap();
    // An error: case 1, the text "the sample is too large" (23 bytes).
    assert_eq!(app.self_test().unwrap(), 2 << 24 | 1 << 16 | 23);
    app.system_sample(3, Err("e".repeat(1000))).unwrap();
    assert_eq!(app.self_test().unwrap(), 3 << 24 | 1 << 16 | 200);
    // Control and bidirectional characters become one `?` each.
    app.system_sample(4, Err("a\u{1b}[31m\u{202e}b".to_string()))
        .unwrap();
    assert_eq!(app.self_test().unwrap(), 4 << 24 | 1 << 16 | 8);
}

#[test]
fn an_app_that_exports_no_samples_is_told_nothing() {
    let mut app = load_core_granted(&guest_with(1, false, false), LIMITS, None, granted()).unwrap();
    assert_eq!(app.system_sample(1, Ok(Vec::new())), Ok(()));
}

/// An app can be given an `Instance` too, through the trait the host uses.
#[test]
fn the_instance_trait_reaches_the_same_queue() {
    let mut app = app_reading(2);
    assert!(Instance::health(&mut app).is_ok());
    assert_eq!(app.take_system(), vec![0, 1]);
}

fn metrics_with(procs: &[(u64, &[u8], u64, u64)]) -> kabi::Metrics {
    let mut metrics = kabi::Metrics::empty();
    metrics.cpus = 4;
    metrics.free_frames = 384_000;
    metrics.total_frames = 1_048_576;
    metrics.heap_bytes = 839_680;
    metrics.uptime_ns = 754_000_000_000;
    metrics.process_count = procs.len() as u64;
    for (slot, (pid, name, pages, cpu_ns)) in metrics.processes.iter_mut().zip(procs) {
        slot.pid = *pid;
        slot.name_len = name.len() as u64;
        slot.name[..name.len()].copy_from_slice(name);
        slot.pages = *pages;
        slot.cpu_ns = *cpu_ns;
    }
    metrics
}

#[test]
fn the_kernels_metrics_become_a_sample() {
    let metrics = metrics_with(&[(1, b"supervisor", 75, 5_000_000), (4, b"llm", 600_000, 9)]);
    let model = systemabi::ModelStatus {
        state: systemabi::ModelState::Ready,
        tps10: 85,
    };
    let sample = crate::system::sample_of(&metrics, model);
    assert_eq!(
        (
            sample.uptime_ns,
            sample.cpus,
            sample.free_pages,
            sample.total_pages
        ),
        (754_000_000_000, 4, 384_000, 1_048_576)
    );
    assert_eq!(sample.heap_bytes, 839_680);
    assert_eq!(sample.omitted, 0);
    assert_eq!(sample.model, model);
    let listed: Vec<_> = sample
        .procs
        .iter()
        .map(|p| (p.pid, p.name.as_str(), p.pages, p.cpu_ns))
        .collect();
    assert_eq!(
        listed,
        [(1, "supervisor", 75, 5_000_000), (4, "llm", 600_000, 9)]
    );
    // What the host writes is what the monitor reads.
    assert_eq!(systemabi::Sample::parse(&sample.encode()), Ok(sample));
}

#[test]
fn metrics_with_absurd_numbers_still_make_a_sample_that_parses() {
    let mut metrics = metrics_with(&[(u64::MAX, b"\xff\xfe not text", u64::MAX, u64::MAX)]);
    metrics.cpus = u64::MAX;
    metrics.process_count = u64::MAX;
    metrics.processes_omitted = u64::MAX;
    metrics.processes[1].name_len = u64::MAX;
    let sample = crate::system::sample_of(&metrics, systemabi::ModelStatus::UNKNOWN);
    assert_eq!(sample.procs.len(), kabi::MAX_METRIC_PROCESSES);
    assert_eq!(sample.procs[0].pid, u32::MAX);
    assert_eq!(sample.procs[0].name, "?");
    assert_eq!(sample.cpus, u32::MAX);
    assert_eq!(sample.omitted, u32::MAX);
    assert_eq!(systemabi::Sample::parse(&sample.encode()), Ok(sample));
}

#[test]
fn processes_past_the_kernels_list_are_counted_as_omitted() {
    let mut metrics = metrics_with(&[(1, b"a", 1, 1)]);
    metrics.process_count = kabi::MAX_METRIC_PROCESSES as u64 + 3;
    metrics.processes_omitted = 2;
    let sample = crate::system::sample_of(&metrics, systemabi::ModelStatus::UNKNOWN);
    assert_eq!(sample.procs.len(), kabi::MAX_METRIC_PROCESSES);
    assert_eq!(sample.omitted, 5);
}

#[test]
fn a_waiting_read_is_seen_without_taking_it() {
    let mut app = app_reading(1);
    assert!(!app.system_waiting());
    app.health().unwrap();
    assert!(app.system_waiting());
    assert!(app.system_waiting(), "looking does not take it");
    assert_eq!(app.take_system(), vec![0]);
    assert!(!app.system_waiting());
}
