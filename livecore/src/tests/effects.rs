//! The per-call transaction and checkpoint undo (ADR 0003 section 4), and S03-1's
//! measure: over 1,000 calls with 10% injected candidate faults, no effect is
//! performed twice or by the version whose reply was not sent, and every own write
//! that commits is the checkpoint's version's.

extern crate std;

use super::fake::{Bug, INJECT, Spec};
use super::{call, host_with, kernel, ledger};
use crate::Reason;
use crate::state::split;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard};

/// Every test here runs alone: [`INJECT`] is global, and versions `a` and `b`
/// trap on `kadd` while any test has it set, so a test that never sets it would
/// still see another's (a CI flake of `the_storage_survives...`).
static INJECTING: Mutex<()> = Mutex::new(());

fn alone() -> MutexGuard<'static, ()> {
    INJECTING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const A: Spec = Spec {
    bug: Bug::Injected,
    ..Spec::new("a", 1)
};
const B: Spec = Spec {
    bug: Bug::Injected,
    ..Spec::new("b", 1)
};

fn stored(kernel: &crate::local::Kernel, key: &str) -> Option<String> {
    let (_, storage) = split(&ledger(kernel).checkpoint).unwrap();
    storage
        .get(key)
        .map(|value| String::from_utf8(value.clone()).unwrap())
}

/// A reply's result, effect lines and undo entry.
fn parts(reply: &str) -> (String, Vec<String>, Option<u64>) {
    let mut lines = reply.lines();
    assert_eq!(lines.next(), Some("kai-reply 1"), "{reply}");
    let result = lines.next().unwrap().to_string();
    let (mut effects, mut undo) = (Vec::new(), None);
    for line in lines {
        if let Some(effect) = line.strip_prefix("effect ") {
            effects.push(effect.to_string());
        } else if let Some(id) = line.strip_prefix("undo ") {
            undo = Some(id.parse().unwrap());
        } else {
            panic!("unexpected line {line:?} in {reply:?}");
        }
    }
    (result, effects, undo)
}

#[test]
fn a_call_commits_its_writes_with_the_checkpoint_and_sends_its_effects() {
    let _guard = alone();
    let kernel = kernel(&[A]);
    let mut host = host_with(&kernel, 10, "version=a probation_calls=3 probation_ms=0");
    let (result, effects, undo) = parts(&call(&mut host, "kadd"));
    assert_eq!(result, "result \"a 1\"");
    assert_eq!(effects, ["1 notify(\"a 1\")"]);
    assert_eq!(undo, Some(1));
    assert_eq!(stored(&kernel, "count").as_deref(), Some("1"));
    // The next call reads what the last one wrote, and its first ticket follows.
    assert!(call(&mut host, "kget").starts_with("kai-reply 1\nresult \"1 by a seed "));
    let (_, effects, _) = parts(&call(&mut host, "kadd"));
    assert_eq!(effects, ["2 notify(\"a 2\")"]);
    // A call that queues too many effects is the version's fault: nothing commits.
    let checkpoints = ledger(&kernel).counters.checkpoints;
    let spam = call(&mut host, "kspam");
    assert!(
        spam.starts_with("err counter failed: abi: it queued 9 effects"),
        "{spam}"
    );
    assert_eq!(ledger(&kernel).counters.checkpoints, checkpoints);
    assert_eq!(stored(&kernel, "count").as_deref(), Some("2"));
}

#[test]
fn the_storage_survives_a_restart_with_the_checkpoint() {
    let _guard = alone();
    let kernel = kernel(&[A]);
    let mut host = host_with(&kernel, 10, "version=a probation_calls=3 probation_ms=0");
    call(&mut host, "kadd");
    call(&mut host, "kadd");
    drop(host);
    let mut host = host_with(&kernel, 11, "version=a probation_calls=3 probation_ms=0");
    assert!(call(&mut host, "kget").starts_with("kai-reply 1\nresult \"2 by a seed "));
    assert_eq!(ledger(&kernel).counters.restores, 2);
}

#[test]
fn undo_restores_the_state_before_a_call_unless_it_changed_since() {
    let _guard = alone();
    let kernel = kernel(&[A]);
    let mut host = host_with(&kernel, 10, "version=a probation_calls=3 probation_ms=0");
    let (_, _, first) = parts(&call(&mut host, "kadd"));
    let (_, _, second) = parts(&call(&mut host, "kadd"));
    let (first, second) = (first.unwrap(), second.unwrap());
    // The state moved on since the first call: undoing it asks first.
    assert_eq!(
        call(&mut host, &std::format!("kai-undo 1\nid {first}\n")),
        std::format!("kai-reply 1\nchanged {first}\n")
    );
    let undone = call(&mut host, &std::format!("kai-undo 1\nid {second}\n"));
    assert_eq!(
        undone,
        std::format!("kai-reply 1\nundone {second}\nundo 3\n")
    );
    assert_eq!(stored(&kernel, "count").as_deref(), Some("1"));
    assert!(call(&mut host, "get").starts_with("ok 1 "));
    // Undoing the undo redoes.
    call(&mut host, "kai-undo 1\nid 3\n");
    assert_eq!(stored(&kernel, "count").as_deref(), Some("2"));
    // With force, a changed state is undone anyway.
    let forced = call(&mut host, &std::format!("kai-undo 1\nid {first}\nforce\n"));
    assert!(forced.starts_with(&std::format!("kai-reply 1\nundone {first}\n")));
    assert_eq!(stored(&kernel, "count"), None);
    assert!(call(&mut host, "get").starts_with("ok 0 "));
    assert!(call(&mut host, "kai-undo 1\nid 99\n").starts_with("kai-reply 1\nrefused no undo 99"));
    assert!(call(&mut host, "kai-undo 1\nid\n").starts_with("kai-reply 1\nrefused "));
}

#[test]
fn a_committed_swap_clears_the_undo_history() {
    let _guard = alone();
    let kernel = kernel(&[A, B]);
    let mut host = host_with(&kernel, 10, "version=a probation_calls=1 probation_ms=0");
    call(&mut host, "kadd");
    assert!(call(&mut host, "ctl swap b").starts_with("ok probation b"));
    // Probation: an undo waits.
    assert!(call(&mut host, "kai-undo 1\nid 1\n").contains("refused an undo waits"));
    call(&mut host, "kadd");
    assert_eq!(ledger(&kernel).active(), Some("b"));
    assert!(call(&mut host, "kai-undo 1\nid 1\n").contains("refused no undo 1"));
}

#[test]
fn on_probation_the_standby_writes_and_the_replying_version_performs() {
    let _guard = alone();
    let kernel = kernel(&[A, B]);
    let mut host = host_with(&kernel, 10, "version=a probation_calls=3 probation_ms=0");
    call(&mut host, "kadd");
    assert!(call(&mut host, "ctl swap b").starts_with("ok probation b"));
    let (result, effects, _) = parts(&call(&mut host, "kadd"));
    assert_eq!(result, "result \"b 2\"");
    assert_eq!(effects, ["2 notify(\"b 2\")"]);
    assert_eq!(stored(&kernel, "by").as_deref(), Some("a"));
    INJECT.store(true, Ordering::SeqCst);
    let reply = call(&mut host, "kadd");
    INJECT.store(false, Ordering::SeqCst);
    let (result, effects, _) = parts(&reply);
    assert_eq!(result, "result \"a 3\"");
    assert_eq!(effects, ["3 notify(\"a 3\")"]);
    assert_eq!(ledger(&kernel).is_bad("b"), Some(Reason::Trap));
}

#[test]
fn s03_1_effects_are_performed_once_by_the_replying_version_over_1000_calls() {
    let _guard = alone();
    const CALLS: u64 = 1000;
    let kernel = kernel(&[A, B]);
    let mut host = host_with(&kernel, 10, "version=a probation_calls=3 probation_ms=0");
    let mut tickets = BTreeSet::new();
    let (mut injected, mut twice, mut wrong_version, mut wrong_writer, mut swaps) = (0, 0, 0, 0, 0);
    for n in 0..CALLS {
        let before = ledger(&kernel);
        let standby = before.active().unwrap().to_string();
        let candidate = match before.candidate() {
            Some(candidate) => candidate.to_string(),
            None => {
                let other = if standby == "a" { "b" } else { "a" };
                let reply = call(&mut host, &std::format!("ctl swap {other} force"));
                assert!(reply.starts_with("ok probation"), "{reply}");
                swaps += 1;
                other.to_string()
            }
        };
        let inject = n % 10 == 0;
        injected += u64::from(inject);
        INJECT.store(inject, Ordering::SeqCst);
        let reply = call(&mut host, "kadd");
        INJECT.store(false, Ordering::SeqCst);
        let (result, effects, _) = parts(&reply);
        let replied = if inject { &standby } else { &candidate };
        if !result.starts_with(&std::format!("result \"{replied} ")) {
            wrong_version += 1;
        }
        for effect in &effects {
            let (ticket, what) = effect.split_once(' ').unwrap();
            if !tickets.insert(ticket.parse::<u64>().unwrap()) {
                twice += 1;
            }
            if !what.starts_with(&std::format!("notify(\"{replied} ")) {
                wrong_version += 1;
            }
        }
        if effects.len() != 1 {
            twice += 1;
        }
        // The writes that committed are the standby's, the checkpoint's version.
        if stored(&kernel, "by").as_deref() != Some(standby.as_str()) {
            wrong_writer += 1;
        }
    }
    let ledger = ledger(&kernel);
    std::println!(
        "S03-1 effects: {CALLS} calls, {swaps} swaps, {injected} injected candidate faults \
         ({} trap rollbacks): {twice} effects performed twice or missing, {wrong_version} from \
         the version whose reply was not sent, {wrong_writer} own writes not from the \
         checkpoint's version",
        ledger.counters.by_reason[Reason::Trap as usize]
    );
    assert_eq!(injected, CALLS / 10);
    assert_eq!(ledger.counters.by_reason[Reason::Trap as usize], injected);
    assert_eq!((twice, wrong_version, wrong_writer), (0, 0, 0));
    assert_eq!(tickets.len() as u64, CALLS);
    assert_eq!(tickets.last().copied(), Some(CALLS));
    assert_eq!(stored(&kernel, "count"), Some(CALLS.to_string()));
}
