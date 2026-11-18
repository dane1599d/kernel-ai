//! An agent's step through the one door (ADR 0013 §1): only `agent-step`
//! from an agent's badged route, admitted by its grants, decided with no
//! borrowed consent, stamped with its name, and an ask sent to the person.

use super::*;
use crate::agentstep::{AGENTS_ONLY_STEP, STEP_NO_HANDLES};

/// `notes-agent`, which may call `cpus` and `set_policy` and nothing else.
fn manifest() -> kaiagent::Manifest {
    kaiagent::Manifest::parse(
        "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass background\n\
         model tokens/day=50000 kv=1024 pool=64m\nactions cpus set_policy\n\
         memory none\nnet none\n",
    )
    .unwrap()
}

/// A shell with `notes-agent` installed, and its route's badge.
fn with_agent() -> (Rc<FakeHost>, Shell, u64) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = shell.roster_mut().install(manifest()).unwrap();
    (host, shell, u64::from(id.badge()))
}

const CPUS: &str = r#"{"name":"cpus","arguments":{}}"#;
const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
const STOP: &str = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;

fn step(shell: &mut Shell, badge: u64, call: &str) -> String {
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

/// `ops-agent`, which may stop processes and set the policy.
fn ops_manifest() -> kaiagent::Manifest {
    kaiagent::Manifest::parse(
        "kai-app 1\nid ops-agent\nabi 1.0\nagent 1\nclass background\n\
         model tokens/day=50000 kv=1024 pool=64m\nactions process_stop set_policy\n\
         memory none\nnet none\n",
    )
    .unwrap()
}

/// The card the terminal takes for the agent's asked step.
fn take_agents_card(shell: &mut Shell) -> termcore::menu::Menu {
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    termcore::menu::Menu::decode(&card.to_wire()).expect("the agent's card")
}

#[test]
fn an_agents_card_names_it_offers_no_always_and_its_allow_runs_once() {
    let (host, mut shell, badge) = with_agent();
    let asked = step(&mut shell, badge, RR);
    assert!(asked.starts_with("ok asked "), "{asked}");
    let menu = take_agents_card(&mut shell);
    assert!(
        menu.question().starts_with("notes-agent asks: "),
        "{menu:?}"
    );
    assert_eq!(
        menu.rows().len(),
        3,
        "Allow and Deny only: {:?}",
        menu.rows()
    );
    let once = host.mint(menu.id, menu.call());
    let ran = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(ran.ok, "{ran:?}");
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    let told = step(&mut shell, badge, &format!("wait {}", menu.id));
    assert!(told.starts_with(&format!("ok ran #{}", menu.id)), "{told}");
    // Once: the same answer again runs nothing.
    host.policy.set(None);
    let again = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(!again.ok, "{again:?}");
    assert_eq!(host.policy.get(), None);
    // The entry says whose it was.
    assert_eq!(
        shell.log.get(menu.id).unwrap().agent.as_str(),
        "notes-agent"
    );
}

#[test]
fn an_agents_question_runs_on_no_answer_but_the_persons_allow_at_its_card() {
    let (host, mut shell, badge) = with_agent();
    step(&mut shell, badge, RR);
    let menu = take_agents_card(&mut shell);
    // A bare y at the console skips the card: it lets the question go.
    let typed = shell.eval_console("y");
    assert!(!typed.ok, "{typed:?}");
    assert_eq!(host.policy.get(), None);
    let told = step(&mut shell, badge, &format!("wait {}", menu.id));
    assert!(
        told.starts_with(&format!("err not run #{}", menu.id)),
        "{told}"
    );
}

#[test]
fn an_always_answer_to_an_agents_card_writes_no_rule_and_runs_nothing() {
    let (host, mut shell, badge) = with_agent();
    step(&mut shell, badge, RR);
    let menu = take_agents_card(&mut shell);
    let once = host.mint(menu.id, menu.call());
    let reply = shell.eval_console(&format!("answer {} always-kind once={once}", menu.id));
    assert!(reply.text.contains("has no Always"), "{reply:?}");
    assert_eq!(host.policy.get(), None);
    assert!(shell.eval("rules").text.starts_with("no rules"));
}

#[test]
fn the_persons_rule_covers_kais_lines_not_an_agents_step() {
    let stop3 = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;
    let stop4 = r#"{"name":"process_stop","arguments":{"name":"build-4"}}"#;
    let host = FakeHost::new();
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(stop3).unwrap()));
    let mut shell = shell(&host);
    // The person stops build-3 and says: always for stopping processes.
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    shell.eval(&format!("answer 1 always-kind once={once}"));
    assert_eq!(shell.eval("rules").text, "#1 process_stop");
    let id = shell.roster_mut().install(ops_manifest()).unwrap();
    let badge = u64::from(id.badge());
    // The agent's stop of build-4 still asks: the rule was the person's.
    let asked = step(&mut shell, badge, stop4);
    assert!(asked.starts_with("ok asked "), "{asked}");
    assert!(host.procs.borrow().iter().any(|p| p == "build-4"));
}

#[test]
fn a_yes_for_a_retired_agent_runs_nothing() {
    let (host, mut shell, badge) = with_agent();
    step(&mut shell, badge, RR);
    let menu = take_agents_card(&mut shell);
    let id = shell.roster_mut().by_name("notes-agent").unwrap().id();
    shell.roster_mut().retire(id).unwrap();
    let once = host.mint(menu.id, menu.call());
    let reply = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(reply.text.contains("grants or call changed"), "{reply:?}");
    assert_eq!(host.policy.get(), None);
}

#[test]
fn agents_take_at_most_two_places_on_the_card_and_the_person_keeps_room() {
    let stop3 = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;
    let host = FakeHost::new();
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(stop3).unwrap()));
    let mut shell = shell(&host);
    let id = shell.roster_mut().install(manifest()).unwrap();
    let badge = u64::from(id.badge());
    for _ in 0..2 {
        assert!(step(&mut shell, badge, RR).starts_with("ok asked "));
    }
    let third = step(&mut shell, badge, RR);
    assert_eq!(
        third,
        "err not asked: agents' questions fill their places on the card"
    );
    // The refused question is logged as the agent's, not the person's.
    let refused = shell.log.last_id();
    assert_eq!(
        shell.log.get(refused).unwrap().agent.as_str(),
        "notes-agent"
    );
    // The person's panel line still reaches the card.
    assert_eq!(
        shell.eval_panel("stop build-3"),
        crate::PanelResult::Done(String::from(crate::deskask::ON_CARD))
    );
}
#[test]
fn the_persons_line_while_an_agents_question_waits_stays_the_persons() {
    let (host, mut shell, badge) = with_agent();
    let asked = step(&mut shell, badge, RR);
    let asked_id: u64 = asked["ok asked ".len()..]
        .split(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    // The person runs a line of their own before the card is taken.
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(RR).unwrap()));
    assert!(shell.eval("switch to round robin").ok);
    let my_id = shell.log.last_id();
    assert!(my_id > asked_id);
    let menu = take_agents_card(&mut shell);
    assert_eq!(menu.id, asked_id);
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert_eq!(
        shell.log.get(menu.id).unwrap().agent.as_str(),
        "notes-agent"
    );
    assert!(
        shell.log.get(my_id).unwrap().agent.is_kai(),
        "the person's own"
    );
}

#[test]
fn a_deny_is_the_agents_entry_and_frees_its_place() {
    let (host, mut shell, badge) = with_agent();
    for _ in 0..3 {
        step(&mut shell, badge, RR);
        let menu = take_agents_card(&mut shell);
        let once = host.mint(menu.id, menu.call());
        let denied = shell.eval_console(&format!("answer {} deny once={once}", menu.id));
        assert!(denied.text.contains("you said Don't"), "{denied:?}");
        assert_eq!(host.policy.get(), None);
        assert_eq!(
            shell.log.get(menu.id).unwrap().agent.as_str(),
            "notes-agent"
        );
        let told = step(&mut shell, badge, &format!("wait {}", menu.id));
        assert!(
            told.starts_with(&format!("err not run #{}", menu.id)),
            "{told}"
        );
    }
    // Each answer gave its places back: the agent can still ask, and its
    // Allow runs.
    step(&mut shell, badge, RR);
    let menu = take_agents_card(&mut shell);
    let once = host.mint(menu.id, menu.call());
    assert!(
        shell
            .eval_console(&format!("answer {} allow once={once}", menu.id))
            .ok
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

#[test]
fn a_lapsed_agents_question_is_its_entry_and_its_wait_hears_so() {
    let (host, mut shell, badge) = with_agent();
    let asked = step(&mut shell, badge, RR);
    let id: u64 = asked["ok asked ".len()..]
        .split(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    host.step_ns.set(200_000_000_000);
    shell.eval_console(termcore::menu::TAKE_CARD);
    host.step_ns.set(1000);
    assert_eq!(shell.log.get(id).unwrap().agent.as_str(), "notes-agent");
    let told = step(&mut shell, badge, &format!("wait {id}"));
    assert!(told.starts_with(&format!("err not run #{id}")), "{told}");
    // Its places came back.
    for _ in 0..2 {
        assert!(step(&mut shell, badge, RR).starts_with("ok asked "));
    }
}

#[test]
fn an_agents_step_never_takes_the_place_of_the_persons_question() {
    let stop3 = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;
    let host = FakeHost::new();
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(stop3).unwrap()));
    let mut shell = shell(&host);
    let id = shell.roster_mut().install(manifest()).unwrap();
    let badge = u64::from(id.badge());
    let mine = shell.eval_console("stop build-3");
    assert!(mine.to_wire().starts_with("menu "), "{mine:?}");
    let mine_id = shell.pending.as_ref().unwrap().id;
    assert_eq!(
        step(&mut shell, badge, RR),
        format!("err {}", crate::agentstep::PERSON_ASKED)
    );
    // The person's question is still theirs to answer.
    let once = host.mint(mine_id, "process_stop(\"build-3\")");
    let done = shell.eval_console(&format!("answer {mine_id} allow once={once}"));
    assert!(done.ok, "{done:?}");
    assert!(!host.procs.borrow().iter().any(|p| p == "build-3"));
}

#[test]
fn a_row_the_call_turns_into_is_admitted_against_the_agents_grants() {
    // `ops-agent` holds process_stop, not service_stop: stopping a service
    // by the process row runs service_stop, so the step is refused.
    let host = FakeHost::new();
    host.services
        .borrow_mut()
        .push((String::from("indexer"), true));
    host.lists_services.set(true);
    let mut shell = shell(&host);
    let id = shell.roster_mut().install(ops_manifest()).unwrap();
    let badge = u64::from(id.badge());
    let stop = r#"{"name":"process_stop","arguments":{"name":"indexer"}}"#;
    let wire = step(&mut shell, badge, stop);
    assert!(wire.contains("it runs service_stop"), "{wire}");
    assert!(!shell.desk_ask_waiting(), "nothing asked");
    let refused = shell.log.last_id();
    assert_eq!(shell.log.get(refused).unwrap().agent.as_str(), "ops-agent");
}
