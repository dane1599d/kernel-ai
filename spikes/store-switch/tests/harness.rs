//! Spike S11-2's host half (ADR 0011): 10,000 random sequences on each of three
//! stores, crashed at every write, flush and commit. `S11_2_SEQUENCES` sets fewer
//! for a quick local run.

use spike_store_switch::harness::{self, Report};
use spike_store_switch::store::Layout;
use std::time::Instant;

fn sequences() -> u64 {
    std::env::var("S11_2_SEQUENCES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(10_000)
}

fn timed(name: &str, run: impl FnOnce() -> Report) -> Report {
    let start = Instant::now();
    let report = run();
    report.print(name);
    println!("  took {:.1} s", start.elapsed().as_secs_f64());
    report
}

/// The decision: ADR 0011's switch (generation, `current`, grants and the log
/// line) as one kaifs transaction, crashed at every write and flush of every
/// sequence.
#[test]
fn one_kaifs_transaction_keeps_the_store_consistent() {
    let report = timed(
        "kaifs, ADR 0011's layout, one transaction per switch",
        || harness::kaifs(harness::store(Layout::Separate), sequences(), 1),
    );
    assert_eq!(report.violations, 0);
    assert!(report.tampered > 0);
    assert_eq!(report.tamper_refused, report.tampered);
}

/// The same switch over design C's three primitives, with no transaction across
/// files: a crash between its renames must show, or the harness could not see a
/// split switch.
#[test]
fn the_same_switch_on_three_primitives_splits() {
    let report = timed("design C's primitives, ADR 0011's layout", || {
        harness::prims(harness::store(Layout::Separate), sequences())
    });
    assert!(report.violations > 0);
}

/// Design C's own layout, grants and log inside the generation, over its three
/// primitives: renaming `current` is the switch.
#[test]
fn design_c_layout_holds_on_three_primitives() {
    let report = timed("design C's primitives, design C's layout", || {
        harness::prims(harness::store(Layout::InGeneration), sequences())
    });
    assert_eq!(report.violations, 0);
    assert_eq!(report.tamper_refused, report.tampered);
}

/// The harness must see a switch split across two kaifs transactions.
#[test]
fn a_switch_split_in_two_transactions_is_caught() {
    let store = spike_store_switch::store::Store {
        split_switch: true,
        ..harness::store(Layout::Separate)
    };
    let report = timed(
        "kaifs, the switch split in two transactions (deliberate fault)",
        || harness::kaifs(store, sequences() / 10, 1),
    );
    assert!(report.violations > 0);
}
