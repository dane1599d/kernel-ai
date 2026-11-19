//! The `maint` agent's rules: a crash loop proposes once, a cooldown holds it,
//! a fault proposes, bad names and floods are bounded, and nothing is an action.

use super::*;
use alloc::string::ToString;
use alloc::vec::Vec;

const MIN: u64 = 60_000;

fn restart(service: &str, at_ms: u64) -> Signal {
    Signal::Restart {
        service: service.to_string(),
        at_ms,
    }
}

#[test]
fn three_restarts_in_ten_minutes_propose_a_look_once() {
    let mut watch = Watch::new();
    assert_eq!(watch.observe(restart("llm", 0)), None);
    assert_eq!(watch.observe(restart("llm", 2 * MIN)), None);
    let proposal = watch
        .observe(restart("llm", 4 * MIN))
        .expect("a crash loop");
    assert_eq!(proposal.why, Why::CrashLoop);
    assert_eq!(proposal.read, ":fault llm");
    assert_eq!(
        proposal.line,
        "llm restarted 3 times in 10 minutes. Look at why with :fault llm?"
    );
    // The loop goes on, and the cooldown holds the next proposal back for an hour.
    for at in 5..60 {
        assert_eq!(watch.observe(restart("llm", at * MIN)), None, "{at}");
    }
    assert!(watch.observe(restart("llm", 65 * MIN)).is_some());
}

#[test]
fn restarts_spread_out_are_no_loop() {
    let mut watch = Watch::new();
    for at in 0..20 {
        assert_eq!(watch.observe(restart("fs", at * 6 * MIN)), None, "{at}");
    }
}

#[test]
fn a_fault_proposes_once_per_cooldown_and_services_are_apart() {
    let mut watch = Watch::new();
    let fault = |service: &str, at_ms| Signal::Fault {
        service: service.to_string(),
        at_ms,
    };
    let p = watch.observe(fault("desktop", 0)).unwrap();
    assert_eq!(p.why, Why::Fault);
    assert_eq!(watch.observe(fault("desktop", MIN)), None);
    assert!(watch.observe(fault("shell", MIN)).is_some());
}

#[test]
fn a_name_out_of_shape_is_not_watched() {
    let mut watch = Watch::new();
    let long = "x".repeat(MAX_NAME + 1);
    for bad in [
        "",
        "LLM",
        "llm service",
        "llm\n:fault shell",
        "llm;rm",
        long.as_str(),
    ] {
        for at in 0..5 {
            assert_eq!(watch.observe(restart(bad, at)), None, "{bad:?}");
        }
    }
    assert_eq!(watch.watched(), 0);
}

#[test]
fn a_clock_that_goes_back_never_widens_the_window() {
    let mut watch = Watch::new();
    assert_eq!(watch.observe(restart("cc", 30 * MIN)), None);
    // An earlier time counts as the last one seen, so these are three in a row.
    assert_eq!(watch.observe(restart("cc", 0)), None);
    assert!(watch.observe(restart("cc", 1)).is_some());
}

#[test]
fn what_is_kept_is_bounded() {
    let mut watch = Watch::new();
    let names: Vec<String> = (0..MAX_SERVICES + 10)
        .map(|n| alloc::format!("svc-{n}"))
        .collect();
    for name in &names {
        watch.observe(restart(name, 0));
    }
    assert_eq!(watch.watched(), MAX_SERVICES);
    // A loop in one service keeps at most LOOP_RESTARTS times.
    for at in 0..1_000 {
        watch.observe(restart("svc-0", at));
    }
    assert!(watch.restarts["svc-0"].len() <= LOOP_RESTARTS);
}
