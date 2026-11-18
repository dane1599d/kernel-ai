//! An agent's step through the one door (ADR 0013 §1): only `agent-step`
//! from an agent's badged route, admitted by its grants, decided with no
//! borrowed consent, stamped with its name, and an ask sent to the person.

use super::*;
use crate::agentstep::{AGENTS_ONLY_STEP, STEP_NO_HANDLES};

/// `notes-agent`, which may call `cpus` and `set_policy` and nothing else.
pub(super) fn manifest() -> kaiagent::Manifest {
    kaiagent::Manifest::parse(
        "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass background\n\
         model tokens/day=50000 kv=1024 pool=64m\nactions cpus set_policy\n\
         memory none\nnet none\n",
    )
    .unwrap()
}

/// A shell with `notes-agent` installed, and its route's badge.
pub(super) fn with_agent() -> (Rc<FakeHost>, Shell, u64) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = shell.roster_mut().install(manifest()).unwrap();
    (host, shell, u64::from(id.badge()))
}

pub(super) const CPUS: &str = r#"{"name":"cpus","arguments":{}}"#;
pub(super) const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
pub(super) const STOP: &str = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;

pub(super) fn step(shell: &mut Shell, badge: u64, call: &str) -> String {
    shell
        .from_route(badge, 0, &format!("agent-step {call}"))
        .expect("an agent's route")
}

#[test]
fn a_read_step_runs_and_its_entry_names_the_agent() {
    let (_, mut shell, badge) = with_agent();
    let wire = step(&mut shell, badge, CPUS);
    assert!(wire.starts_with("ok run #"), "{wire}");
    let id = shell.log.last_id();
    assert_eq!(shell.log.get(id).unwrap().agent.as_str(), "notes-agent");
}

#[test]
fn a_change_step_asks_the_person_and_the_step_waits() {
    let (host, mut shell, badge) = with_agent();
    // The person's words never cue an agent's change: it asks.
    let wire = step(&mut shell, badge, RR);
    assert!(wire.starts_with("ok asked "), "{wire}");
    assert!(shell.desk_ask_waiting(), "on the person's card");
    assert_eq!(host.policy.get(), None, "nothing ran");
    let id = shell.desk_asks[0].pending.id;
    assert_eq!(
        step(&mut shell, badge, &format!("wait {id}")),
        format!("ok waiting #{id}")
    );
}

#[test]
fn a_step_over_the_agents_grants_is_refused() {
    let (host, mut shell, badge) = with_agent();
    let wire = step(&mut shell, badge, STOP);
    assert_eq!(
        wire,
        "err refused: the agent's grants do not include that action"
    );
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|a| a.contains("process_stop refused"))
    );
}

#[test]
fn a_forged_or_kais_badge_steps_nothing() {
    let (_, mut shell, _) = with_agent();
    assert_eq!(step(&mut shell, 999, CPUS), "err that route is no agent's");
    assert_eq!(
        step(&mut shell, u64::MAX, CPUS),
        "err that route is no agent's"
    );
    let kai = u64::from(shell.roster_mut().by_name("kai").unwrap().id().badge());
    assert_eq!(
        step(&mut shell, kai, CPUS),
        "err kai's lines are typed, not stepped"
    );
    // Badge 0 is the kernel's: not an agent's route at all.
    assert_eq!(shell.from_route(0, 0, &format!("agent-step {CPUS}")), None);
}

#[test]
fn an_agents_route_may_send_nothing_but_a_step() {
    let (host, mut shell, badge) = with_agent();
    for request in [
        "undo",
        "undo 1",
        "net denied 3",
        "answer 1 allow",
        "cpus()",
        "switch to round robin",
        "\u{1}card",
        "panel take",
        "log actlog",
        "agent-stepcpus",
    ] {
        assert_eq!(
            shell.from_route(badge, 0, request),
            Some(format!("err {AGENTS_ONLY_STEP}")),
            "{request:?}"
        );
    }
    assert_eq!(host.policy.get(), None);
}

#[test]
fn a_step_that_carried_handles_is_refused_and_runs_nothing() {
    let (host, mut shell, badge) = with_agent();
    let before = shell.log.last_id();
    for handles in [1, 4] {
        assert_eq!(
            shell.from_route(badge, handles, &format!("agent-step {RR}")),
            Some(format!("err {STEP_NO_HANDLES}"))
        );
    }
    assert_eq!(host.policy.get(), None, "nothing ran");
    assert!(!shell.desk_ask_waiting(), "nothing asked");
    assert_eq!(shell.log.last_id(), before, "nothing logged");
    // The kernel's own requests are the caller's to answer, handles or not.
    assert_eq!(shell.from_route(0, 1, "cpus()"), None);
}
