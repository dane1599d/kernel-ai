//! The component host against the real store and ledger, with scripted versions.

extern crate std;

mod effects;
mod fake;
mod fallback;
mod rehearsal;
mod saved;
mod suspend;

use crate::Reason;
use crate::host::{EXIT_REFUSED, EXIT_RESTARTS, Host};
use crate::ledger::Ledger;
use crate::local::{Kernel, LocalEnv};
use crate::settings::Settings;
use crate::store::{Bytes, Component};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use fake::{Bug, Fake, Spec};

const SETTINGS: &str = "version=1 probation_calls=3 probation_ms=0";
const V1: Spec = Spec::new("1", 1);
const V2: Spec = Spec {
    reads: &[1],
    ..Spec::new("2", 2)
};

fn broken(label: &'static str, bug: Bug) -> Spec {
    Spec {
        bug,
        ..Spec { label, ..V2 }
    }
}

fn kernel(specs: &[Spec]) -> Kernel {
    let mut component = Component::new("counter");
    for spec in specs {
        component
            .add_version(spec.label, Bytes::shared(spec.artifact()))
            .unwrap();
    }
    Kernel::new(component)
}

type TestHost = Host<LocalEnv<Fake>>;

fn host_with(kernel: &Kernel, pid: u64, settings: &str) -> TestHost {
    let env = kernel.env(pid, Box::new(fake::load));
    Host::start(env, "counter", Settings::parse(settings).unwrap()).unwrap()
}

fn host(kernel: &Kernel) -> TestHost {
    host_with(kernel, 10, SETTINGS)
}

fn call(host: &mut TestHost, request: &str) -> String {
    String::from_utf8(host.handle(0, request.as_bytes()).unwrap()).unwrap()
}

fn ledger(kernel: &Kernel) -> Ledger {
    kernel.component.borrow().ledger.clone()
}

fn reason_count(kernel: &Kernel, reason: Reason) -> u64 {
    ledger(kernel).counters.by_reason[reason as usize]
}

#[test]
fn an_empty_ledger_starts_the_configured_version_fresh() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    assert_eq!(call(&mut host, "get"), "ok 0 schema 1");
    let ledger = ledger(&kernel);
    assert_eq!((ledger.active(), ledger.counters.restores), (Some("1"), 1));
    assert!(
        kernel
            .log
            .borrow()
            .iter()
            .any(|line| line == "live: counter 1 started fresh")
    );
}

#[test]
fn a_live_swap_migrates_the_state_and_commits_after_its_calls() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    assert_eq!(call(&mut host, "add"), "ok 2");
    assert_eq!(ledger(&kernel).counters.checkpoints, 2);
    assert_eq!(call(&mut host, "ctl swap 2"), "ok probation 2 (3 calls)");
    // The candidate answers, holding the migrated count.
    assert_eq!(call(&mut host, "get"), "ok 2 schema 2");
    assert_eq!(
        call(&mut host, "ctl status"),
        "ok counter 1 schema 1; probation 2 schema 2: 1 of 3 calls"
    );
    assert_eq!(call(&mut host, "add"), "ok 3");
    assert_eq!(call(&mut host, "add"), "ok 4");
    let ledger = ledger(&kernel);
    assert_eq!(
        (ledger.active(), ledger.schema, ledger.generation),
        (Some("2"), 2, 1)
    );
    assert_eq!((ledger.counters.commits, ledger.counters.served), (1, 3));
    assert_eq!(ledger.checkpoint, Vec::from([0x82, 2, 4]));
    assert_eq!(call(&mut host, "ctl status"), "ok counter 2 schema 2");
}

#[test]
fn without_probation_a_swap_commits_at_once() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host_with(&kernel, 10, "version=1 probation_calls=0 probation_ms=0");
    assert_eq!(call(&mut host, "ctl swap 2"), "ok committed 2");
    assert_eq!(ledger(&kernel).active(), Some("2"));
}

#[test]
fn a_version_that_does_not_link_rolls_back_and_the_old_one_carries_on() {
    let kernel = kernel(&[V1, broken("3", Bug::Link)]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    assert_eq!(
        call(&mut host, "ctl swap 3"),
        "err rolled back 3: link: unknown import kai.teleport"
    );
    assert_eq!(call(&mut host, "get"), "ok 1 schema 1");
    assert_eq!(reason_count(&kernel, Reason::Link), 1);
    assert_eq!(ledger(&kernel).is_bad("3"), Some(Reason::Link));
}

#[test]
fn a_version_that_cannot_read_the_state_is_no_migration_unless_the_old_one_writes_an_older_schema()
{
    let kernel = kernel(&[V1, V2]);
    let mut host = host_with(&kernel, 10, "version=2 probation_calls=3 probation_ms=0");
    assert_eq!(
        call(&mut host, "ctl swap 1"),
        "err rolled back 1: no-migration: 1 cannot read schema 2 and 2 cannot write schema 1"
    );
    assert_eq!(reason_count(&kernel, Reason::NoMigration), 1);
    assert_eq!(ledger(&kernel).is_bad("1"), None);
    // A version that can write schema 1 lets version 1 take over its state.
    let downgrades = Spec {
        writes: &[1],
        ..Spec { label: "2", ..V2 }
    };
    let kernel = kernel_with_1_and(downgrades);
    let mut host = host_with(&kernel, 10, "version=2 probation_calls=3 probation_ms=0");
    call(&mut host, "add");
    assert_eq!(call(&mut host, "ctl swap 1"), "ok probation 1 (3 calls)");
    assert_eq!(call(&mut host, "get"), "ok 1 schema 1");
}

fn kernel_with_1_and(spec: Spec) -> Kernel {
    kernel(&[V1, spec])
}

#[test]
fn failures_before_probation_name_the_step() {
    for (bug, reason) in [
        (Bug::ImportTrap, Reason::Migrate),
        (Bug::SelfTest, Reason::SelfTest),
        (Bug::Health, Reason::Health),
        (Bug::Export, Reason::Export),
    ] {
        let kernel = kernel(&[V1, broken("3", bug)]);
        let mut host = host(&kernel);
        let reply = call(&mut host, "ctl swap 3");
        assert!(
            reply.starts_with(&alloc::format!("err rolled back 3: {}", reason.name())),
            "{reply}"
        );
        assert_eq!(reason_count(&kernel, reason), 1);
        assert_eq!(ledger(&kernel).active(), Some("1"));
    }
}

#[test]
fn an_incompatible_world_or_a_missing_source_is_a_manifest_failure() {
    let other = Spec {
        world: "app:counter/counter@2.0.0",
        ..Spec { label: "3", ..V2 }
    };
    let mut component = Component::new("counter");
    component
        .add_version("1", Bytes::shared(V1.artifact()))
        .unwrap();
    component
        .add_version("3", Bytes::shared(other.artifact()))
        .unwrap();
    let mut bare = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    crate::artifact::append_custom_section(&mut bare, "fake", b"2 1 - counter@1 None");
    component.add_version("4", Bytes::shared(bare)).unwrap();
    let kernel = Kernel::new(component);
    let mut host = host(&kernel);
    assert!(
        call(&mut host, "ctl swap 3")
            .contains("manifest: world app:counter/counter@2.0.0 is not compatible")
    );
    assert!(call(&mut host, "ctl swap 4").contains("manifest: no kai.manifest"));
    assert_eq!(reason_count(&kernel, Reason::Manifest), 2);
}

#[test]
fn on_probation_the_old_version_runs_first_and_its_state_is_the_checkpoint() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    call(&mut host, "ctl swap 2");
    call(&mut host, "add");
    // Still the active version's state and schema.
    assert_eq!(ledger(&kernel).checkpoint, Vec::from([0x82, 1, 1]));
    assert_eq!(ledger(&kernel).schema, 1);
}

#[test]
fn a_candidate_that_traps_rolls_back_and_the_caller_gets_the_old_reply() {
    let kernel = kernel(&[V1, broken("2", Bug::TrapAt(3))]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    call(&mut host, "ctl swap 2");
    assert_eq!(call(&mut host, "add"), "ok 2");
    assert_eq!(call(&mut host, "add"), "ok 3");
    assert_eq!(call(&mut host, "get"), "ok 3 schema 1");
    let ledger = ledger(&kernel);
    assert_eq!((ledger.counters.rollbacks, ledger.counters.served), (1, 1));
    assert_eq!(ledger.is_bad("2"), Some(Reason::Trap));
    assert!(
        kernel
            .log
            .borrow()
            .iter()
            .any(|line| line.starts_with("live: counter 1 -> 2 rolled back (trap"))
    );
}

#[test]
fn a_request_the_old_version_fails_is_not_given_to_the_candidate() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    call(&mut host, "ctl swap 2");
    assert_eq!(
        call(&mut host, "poison"),
        "err counter failed on this request: trap"
    );
    let ledger = ledger(&kernel);
    assert_eq!((ledger.counters.faults, ledger.counters.restarts), (1, 1));
    assert_eq!(
        (ledger.candidate(), ledger.counters.rollbacks),
        (Some("2"), 0)
    );
    // Both versions still hold what the kernel holds.
    assert_eq!(call(&mut host, "get"), "ok 1 schema 2");
    call(&mut host, "ctl rollback");
    assert_eq!(call(&mut host, "get"), "ok 1 schema 1");
}

#[test]
fn a_write_the_old_version_fails_is_not_acknowledged_and_lost() {
    // Version 1 traps on its second add; version 2 is the fix.
    let kernel = kernel(&[broken_v1(Bug::TrapAt(2)), V2]);
    let mut host = host(&kernel);
    call(&mut host, "ctl swap 2");
    assert_eq!(call(&mut host, "add"), "ok 1");
    assert_eq!(
        call(&mut host, "add"),
        "err counter failed on this request: trap"
    );
    assert_eq!(ledger(&kernel).checkpoint, Vec::from([0x82, 1, 1]));
    call(&mut host, "ctl rollback");
    assert_eq!(call(&mut host, "get"), "ok 1 schema 1");
}

#[test]
fn an_old_version_that_keeps_failing_ends_the_swap_without_blaming_the_candidate() {
    let kernel = kernel(&[broken_v1(Bug::TrapAt(1)), V2]);
    let mut host = host(&kernel);
    call(&mut host, "ctl swap 2");
    for _ in 0..3 {
        call(&mut host, "add");
    }
    assert_eq!(host.handle(0, b"add").unwrap_err().code, EXIT_RESTARTS);
    let ledger = ledger(&kernel);
    assert_eq!((ledger.active(), ledger.candidate()), (Some("1"), None));
    assert_eq!(ledger.is_bad("2"), None);
}

#[test]
fn an_old_version_that_fails_to_write_an_older_schema_does_not_blame_the_candidate() {
    let writer = Spec {
        writes: &[1],
        bug: Bug::ExportOlderTrap,
        ..Spec { label: "2", ..V2 }
    };
    let kernel = kernel(&[V1, writer]);
    let mut host = host_with(&kernel, 10, "version=2 probation_calls=3 probation_ms=0");
    call(&mut host, "add");
    let reply = call(&mut host, "ctl swap 1");
    assert!(
        reply.starts_with("err rolled back 1: export: 2 failed to write schema 1"),
        "{reply}"
    );
    let ledger = ledger(&kernel);
    assert_eq!(ledger.is_bad("1"), None);
    assert_eq!(ledger.counters.restarts, 1);
    assert_eq!(call(&mut host, "get"), "ok 1 schema 2");
}

#[test]
fn a_candidate_that_fails_its_last_check_does_not_give_the_answer() {
    let kernel = kernel(&[V1, broken("2", Bug::HealthAt(1))]);
    let mut host = host_with(&kernel, 10, "version=1 probation_calls=2 probation_ms=0");
    call(&mut host, "ctl swap 2");
    call(&mut host, "add");
    // The second call ends probation; the candidate's last health check fails.
    assert_eq!(call(&mut host, "get"), "ok 1 schema 1");
    assert_eq!(reason_count(&kernel, Reason::Health), 1);
}

fn broken_v1(bug: Bug) -> Spec {
    Spec { bug, ..V1 }
}

#[test]
fn a_version_that_failed_needs_force() {
    let kernel = kernel(&[V1, broken("2", Bug::TrapAt(1))]);
    let mut host = host(&kernel);
    call(&mut host, "ctl swap 2");
    call(&mut host, "add");
    assert_eq!(
        call(&mut host, "ctl swap 2"),
        "err 2 failed before; use ctl swap 2 force"
    );
    assert_eq!(ledger(&kernel).counters.refused_bad, 1);
    assert_eq!(
        call(&mut host, "ctl swap 2 force"),
        "ok probation 2 (3 calls)"
    );
}

#[test]
fn time_alone_never_rolls_back_and_commits_need_calls_and_time() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host_with(&kernel, 10, "version=1 probation_calls=1 probation_ms=1000");
    call(&mut host, "ctl swap 2");
    call(&mut host, "add");
    kernel.clock.set(500);
    assert!(call(&mut host, "ctl status").contains("probation 2"));
    kernel.clock.set(2000);
    call(&mut host, "get");
    assert_eq!(ledger(&kernel).active(), Some("2"));
    assert_eq!(ledger(&kernel).counters.rollbacks, 0);
}

#[test]
fn an_idle_candidate_is_committed_only_when_probation_has_a_time() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host_with(&kernel, 10, "version=1 probation_calls=5 probation_ms=1000");
    call(&mut host, "ctl swap 2");
    assert_eq!(host.timeout_ms(), 1000);
    host.tick().unwrap();
    kernel.clock.set(4000);
    host.tick().unwrap();
    assert_eq!(ledger(&kernel).active(), Some("2"));
    assert_eq!(host.timeout_ms(), 0);
    let calls_only = crate::tests::kernel(&[V1, V2]);
    let mut by_calls = crate::tests::host(&calls_only);
    call(&mut by_calls, "ctl swap 2");
    calls_only.clock.set(1_000_000);
    for _ in 0..5 {
        by_calls.tick().unwrap();
    }
    assert_eq!(ledger(&calls_only).active(), Some("1"));
}

#[test]
fn a_failed_health_check_while_idle_rolls_back() {
    let kernel = kernel(&[V1, broken("2", Bug::HealthAt(2))]);
    let mut host = host(&kernel);
    call(&mut host, "ctl swap 2");
    host.tick().unwrap();
    assert!(call(&mut host, "ctl status").contains("probation"));
    call(&mut host, "add");
    call(&mut host, "add");
    host.tick().unwrap();
    assert_eq!(reason_count(&kernel, Reason::Health), 1);
    assert_eq!(ledger(&kernel).active(), Some("1"));
}

#[test]
fn a_manual_rollback_is_not_held_against_the_version_and_a_second_swap_waits() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    call(&mut host, "ctl swap 2");
    assert_eq!(
        call(&mut host, "ctl swap 2"),
        "err busy: a version is on probation"
    );
    assert_eq!(call(&mut host, "ctl rollback"), "ok rolled back 2");
    assert_eq!(
        call(&mut host, "ctl rollback"),
        "err nothing is on probation"
    );
    assert_eq!(reason_count(&kernel, Reason::Manual), 1);
    assert_eq!(ledger(&kernel).is_bad("2"), None);
}

#[test]
fn regressions_are_counted_but_do_not_roll_back() {
    let kernel = kernel(&[V1, broken("2", Bug::Regress)]);
    let mut host = host(&kernel);
    call(&mut host, "ctl swap 2");
    for _ in 0..3 {
        assert_eq!(call(&mut host, "add"), "err no");
    }
    let ledger = ledger(&kernel);
    assert_eq!(
        (ledger.active(), ledger.counters.regressions),
        (Some("2"), 3)
    );
}

#[test]
fn control_requests_come_only_from_the_kernel() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    let reply = host.handle(42, b"ctl swap 2").unwrap();
    assert_eq!(reply, b"err control requests come only from the kernel");
    assert_eq!(ledger(&kernel).counters.begins, 0);
    assert_eq!(
        call(&mut host, "ctl dance"),
        alloc::format!("err {}", crate::control::USAGE)
    );
}

#[test]
fn a_request_that_changes_nothing_sends_no_checkpoint() {
    let kernel = kernel(&[V1]);
    let mut host = host(&kernel);
    call(&mut host, "get");
    call(&mut host, "get");
    assert_eq!(ledger(&kernel).counters.checkpoints, 0);
}

#[test]
fn a_fault_while_serving_reloads_from_the_checkpoint_until_too_many() {
    let kernel = kernel(&[V1]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    let reply = call(&mut host, "poison");
    assert!(reply.starts_with("err counter failed: trap: "), "{reply}");
    assert_eq!(call(&mut host, "get"), "ok 1 schema 1");
    // Spread out, faults never end the host.
    for minute in 1..5 {
        kernel.clock.set(minute * 61_000);
        call(&mut host, "poison");
    }
    // Four within a minute do.
    kernel.clock.set(400_000);
    for _ in 0..3 {
        call(&mut host, "poison");
    }
    let exit = host.handle(0, b"poison").unwrap_err();
    assert_eq!(exit.code, EXIT_RESTARTS);
}

#[test]
fn an_event_the_kernel_refuses_ends_the_host() {
    let kernel = kernel(&[V1]);
    let mut host = host(&kernel);
    kernel.component.borrow_mut().ledger.host_pid = 999;
    assert_eq!(host.handle(0, b"add").unwrap_err().code, EXIT_REFUSED);
}

#[test]
fn a_host_that_died_mid_swap_is_rolled_back_by_the_next_one() {
    let kernel = kernel(&[V1, V2]);
    let mut first = host(&kernel);
    call(&mut first, "add");
    call(&mut first, "ctl swap 2");
    call(&mut first, "add");
    drop(first);
    let mut second = host_with(&kernel, 11, SETTINGS);
    assert_eq!(call(&mut second, "get"), "ok 2 schema 1");
    assert_eq!(reason_count(&kernel, Reason::HostRestart), 1);
    let ledger = ledger(&kernel);
    assert_eq!((ledger.active(), ledger.counters.restores), (Some("1"), 2));
}

#[test]
fn a_checkpoint_that_cannot_be_restored_disables_until_reset() {
    let kernel = kernel(&[V1]);
    drop(host(&kernel));
    kernel.component.borrow_mut().ledger.checkpoint = Vec::from([0x82, 1, 0x18, 30]);
    let mut host = host_with(&kernel, 11, SETTINGS);
    let reply = call(&mut host, "add");
    assert!(
        reply.starts_with("err counter disabled: cannot restore its state (migrate"),
        "{reply}"
    );
    assert!(call(&mut host, "ctl swap 2").contains("disabled"));
    assert_eq!(
        call(&mut host, "ctl reset"),
        "ok counter started fresh on version 1"
    );
    assert_eq!(call(&mut host, "get"), "ok 0 schema 1");
    let ledger = ledger(&kernel);
    assert_eq!(
        (
            ledger.disabled,
            ledger.counters.resets,
            ledger.counters.disabled
        ),
        (false, 1, 1)
    );
}

#[test]
fn the_source_of_the_active_version_can_be_listed_and_read() {
    let kernel = kernel(&[V1]);
    let mut host = host(&kernel);
    assert_eq!(
        call(&mut host, "ctl source"),
        "ok counter 1 (0123456789ab):\nsrc/lib.rs 13"
    );
    assert_eq!(
        call(&mut host, "ctl source src/lib.rs"),
        "ok // a counter\n"
    );
    assert_eq!(
        call(&mut host, "ctl source nope"),
        "err no file nope in version 1"
    );
}
