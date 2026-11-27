//! Per-agent consent at the one door (ADR 0013 §1): the agent's card, the
//! person's yes and whose entries are whose (`agentconsent`).

use super::agentstep::{CPUS, RR, STOP, manifest, step, with_agent};
use super::*;

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

/// bug-hunt-6 H6-26: the person allowed the agent's step and it then failed
/// (the process was gone): the wait hears a failed run, which the goals agent
/// bills and logs as one, and not a no, which it would remember and hold a
/// repeat for.
#[test]
fn a_step_that_failed_after_allow_is_told_as_failed_not_as_a_no() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    let mut shell = shell(&host);
    let badge = u64::from(shell.roster_mut().install(ops_manifest()).unwrap().badge());
    let stop = r#"{"name":"process_stop","arguments":{"name":"notes"}}"#;
    assert!(step(&mut shell, badge, stop).starts_with("ok asked "));
    let menu = take_agents_card(&mut shell);
    host.procs.borrow_mut().clear();
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    let told = step(&mut shell, badge, &format!("wait {}", menu.id));
    assert!(
        told.starts_with(&format!("err failed #{}:", menu.id)),
        "{told}"
    );
    assert!(matches!(
        goalcore::reply::read(&told),
        goalcore::reply::Reply::Done(goalcore::StepDone {
            entry: Some(_),
            failed: Some(_),
            ..
        })
    ));
}

/// An allowed run whose file changes did not land is the agent's failed step
/// too: the "not landed" line is no no, so it is not logged as the person's.
#[test]
fn a_run_whose_changes_did_not_land_is_told_as_failed_not_as_a_no() {
    let (_host, mut shell, badge) = with_agent();
    assert!(step(&mut shell, badge, RR).starts_with("ok asked "));
    let menu = take_agents_card(&mut shell);
    shell.pending = None;
    shell.landing.line = Some(String::from("its file changes did not land"));
    shell.desk_answered(Some((menu.id, "allow")), &crate::Reply::err("x"), false);
    let told = step(&mut shell, badge, &format!("wait {}", menu.id));
    assert!(
        told.starts_with(&format!("err failed #{}:", menu.id)),
        "{told}"
    );
}

/// A card nobody answered in two minutes is told as lapsed, not as the
/// person's no, so the goals agent holds the step and does not log a refusal.
#[test]
fn a_card_that_lapsed_is_told_as_lapsed_not_as_a_no() {
    let (_host, mut shell, badge) = with_agent();
    assert!(step(&mut shell, badge, RR).starts_with("ok asked "));
    let menu = take_agents_card(&mut shell);
    shell.pending = None;
    shell.desk_answered(Some((menu.id, "allow")), &crate::Reply::err("x"), true);
    let told = step(&mut shell, badge, &format!("wait {}", menu.id));
    assert!(
        told.starts_with(&format!("err lapsed #{}:", menu.id)),
        "{told}"
    );
}

/// bug-hunt-5 H5-8: while the console is at c>, the person's Allow on an
/// agent's card runs its call as at kai>, never goes to cc as C, and the
/// agent's wait hears that it ran.
#[test]
fn an_agents_card_allowed_at_c_runs_its_call() {
    let (host, mut shell, badge) = with_agent();
    super::cmode::start_cc(&host);
    assert!(shell.eval(":c").ok && shell.at_c());
    let asked = step(&mut shell, badge, RR);
    assert!(asked.starts_with("ok asked "), "{asked}");
    let menu = take_agents_card(&mut shell);
    let once = host.mint(menu.id, menu.call());
    let ran = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(ran.ok, "{ran:?}");
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert!(host.cc_sent.borrow().is_empty(), "{:?}", host.cc_sent);
    let told = step(&mut shell, badge, &format!("wait {}", menu.id));
    assert!(told.starts_with(&format!("ok ran #{}", menu.id)), "{told}");
    assert!(shell.at_c());
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

#[test]
fn a_yes_on_an_agents_forwarded_app_row_runs_it() {
    // ui_open forwards to the editor's own row, which the agent also holds:
    // the person's Allow runs it, checked against the same rows.
    let host = FakeHost::new();
    host.apps
        .borrow_mut()
        .rows
        .load("editor", super::apps::EDITOR)
        .unwrap();
    host.app_replies
        .borrow_mut()
        .push_back(String::from("kai-reply 1\nresult ok(\"opened\")\nundo 2\n"));
    let mut shell = shell(&host);
    let manifest = kaiagent::Manifest::parse(
        "kai-app 1\nid edit-agent\nabi 1.0\nagent 1\nclass background\n\
         model tokens/day=50000 kv=1024 pool=64m\nactions ui_open editor_open_file\n\
         memory none\nnet none\n",
    )
    .unwrap();
    let id = shell.roster_mut().install(manifest).unwrap();
    let badge = u64::from(id.badge());
    let open = r#"{"name":"ui_open","arguments":{"app":"editor","arg":"ideas.txt"}}"#;
    let asked = step(&mut shell, badge, open);
    assert!(asked.starts_with("ok asked "), "{asked}");
    let menu = take_agents_card(&mut shell);
    let once = host.mint(menu.id, menu.call());
    let ran = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(ran.ok, "{ran:?}");
    assert_eq!(host.app_requests.borrow().len(), 1, "the editor's row ran");
}

#[test]
fn the_persons_question_that_lapses_during_an_agents_step_stays_the_persons() {
    let stop3 = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;
    let host = FakeHost::new();
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(stop3).unwrap()));
    let mut shell = shell(&host);
    let id = shell.roster_mut().install(manifest()).unwrap();
    let badge = u64::from(id.badge());
    shell.eval_panel("stop build-3");
    let panel_id = shell.desk_asks[0].pending.id;
    host.step_ns.set(200_000_000_000);
    let asked = step(&mut shell, badge, RR);
    host.step_ns.set(1000);
    assert!(asked.starts_with("ok asked "), "{asked}");
    let lapsed = shell.log.get(panel_id).expect("the lapse is logged");
    assert!(lapsed.agent.is_kai(), "{lapsed:?}");
}

#[test]
fn an_agents_which_one_goes_to_the_agent_not_the_persons_next_line() {
    let (host, mut shell, _) = with_agent();
    host.lists_services.set(true);
    let id = shell.roster_mut().install(ops_manifest()).unwrap();
    let badge = u64::from(id.badge());
    let stop = r#"{"name":"process_stop","arguments":{"name":"no-such"}}"#;
    let wire = step(&mut shell, badge, stop);
    assert!(wire.starts_with("err unclear #"), "{wire}");
    assert!(
        shell.clarifying.is_none(),
        "the console's own state is untouched"
    );
    // While the person's own "which one?" waits, an agent's step waits too.
    shell.clarifying = Some(String::from("stop the build"));
    assert_eq!(
        step(&mut shell, badge, RR),
        format!("err {}", crate::agentstep::PERSON_ASKED)
    );
}

/// A shell whose first model call stops build-3 (a question that asks), with
/// `notes-agent` installed: the host, the shell and the agent's badge.
fn with_agent_and_stop() -> (Rc<FakeHost>, Shell, u64) {
    let stop3 = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;
    let host = FakeHost::new();
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(stop3).unwrap()));
    let mut shell = shell(&host);
    let id = shell.roster_mut().install(manifest()).unwrap();
    (host, shell, u64::from(id.badge()))
}

#[test]
fn only_an_admitted_step_hears_busy_and_it_is_audited() {
    let (host, mut shell, badge) = with_agent_and_stop();
    shell.eval_console("stop build-3");
    assert!(shell.pending.is_some());
    // A row outside the grants is refused as such: it learns nothing.
    assert_eq!(
        step(&mut shell, badge, STOP),
        "err refused: the agent's grants do not include that action"
    );
    assert_eq!(
        step(&mut shell, badge, RR),
        format!("err {}", crate::agentstep::PERSON_ASKED)
    );
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|a| a.contains("notes-agent's step set_policy waits"))
    );
}

#[test]
fn an_agents_step_leaves_the_persons_show_alone() {
    let (host, mut shell, badge) = with_agent();
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(RR).unwrap()));
    shell.eval("switch to round robin");
    let mine = shell.eval("show");
    step(&mut shell, badge, CPUS);
    // Still the person's script (#1064 makes the "by" line follow it too).
    let now = shell.eval("show");
    assert!(now.text.starts_with(&mine.text), "{now:?}");
    assert!(now.text.starts_with("script 1: "), "{now:?}");
}

#[test]
fn an_agents_name_that_makes_the_question_too_long_keeps_it_off_the_card() {
    let question = "q".repeat(termcore::menu::MAX_TEXT - 4);
    let menu = termcore::menu::Menu::encode(
        7,
        termcore::menu::Origin::Added,
        &question,
        "",
        "",
        "cpus()",
    );
    assert_eq!(crate::agentconsent::agent_menu("notes-agent", &menu), None);
    let short = termcore::menu::Menu::encode(
        7,
        termcore::menu::Origin::Added,
        "Run cpus()?",
        "",
        "",
        "cpus()",
    );
    assert!(crate::agentconsent::agent_menu("notes-agent", &short).is_some());
}

#[test]
fn retiring_an_agent_lets_its_waiting_questions_go_at_once() {
    let (_, mut shell, badge) = with_agent();
    let first = step(&mut shell, badge, RR);
    let id: u64 = first["ok asked ".len()..]
        .split(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    step(&mut shell, badge, RR);
    assert_eq!(shell.desk_asks.len(), 2);
    let agent = shell.roster_mut().by_name("notes-agent").unwrap().id();
    shell.retire_agent(agent).unwrap();
    assert!(shell.desk_asks.is_empty(), "their places are free");
    let entry = shell.log.get(id).unwrap();
    assert_eq!(entry.agent.as_str(), "notes-agent");
    // Another agent can ask at once.
    let other = shell.roster_mut().install(ops_manifest()).unwrap();
    let asked = step(&mut shell, u64::from(other.badge()), RR);
    assert!(asked.starts_with("ok asked "), "{asked}");
}

#[test]
fn the_persons_line_reaches_the_card_before_an_agents_question() {
    let (_, mut shell, badge) = with_agent_and_stop();
    step(&mut shell, badge, RR);
    shell.eval_panel("stop build-3");
    let card = take_agents_card(&mut shell);
    assert!(!card.question().starts_with("notes-agent asks"), "{card:?}");
    assert_eq!(card.call(), "process_stop(\"build-3\")");
}

#[test]
fn two_agents_share_the_agents_places_on_the_card() {
    let (_, mut shell, badge) = with_agent();
    let other = shell.roster_mut().install(ops_manifest()).unwrap();
    let other = u64::from(other.badge());
    assert!(step(&mut shell, badge, RR).starts_with("ok asked "));
    assert!(step(&mut shell, other, RR).starts_with("ok asked "));
    for who in [badge, other] {
        assert_eq!(
            step(&mut shell, who, RR),
            "err not asked: agents' questions fill their places on the card"
        );
    }
}

/// An agent retired while its question is on the card: the question is
/// recorded dropped as the agent's, audited, and its place and admission go;
/// the card's stale Allow runs nothing and says why, rather than failing;
/// and a question put on the card after it is untouched by that answer and
/// answers as ever (#1085's follow-up; #1104's review).
#[test]
fn retiring_an_agent_drops_its_question_on_the_card() {
    let (host, mut shell, badge) = with_agent();
    step(&mut shell, badge, RR);
    let menu = take_agents_card(&mut shell);
    assert!(shell.agent_asks.is_open(menu.id));
    assert!(shell.agent_admissions.iter().any(|(e, _)| *e == menu.id));
    let agent = shell.roster_mut().by_name("notes-agent").unwrap().id();
    shell.retire_agent(agent).unwrap();
    assert!(shell.pending.is_none());
    let entry = shell.log.get(menu.id).unwrap();
    assert!(
        entry.outcome.contains("dropped: the agent was retired"),
        "{entry:?}"
    );
    assert_eq!(entry.agent.as_str(), "notes-agent");
    let audited = |text: &str| host.audit.borrow().iter().any(|line| line.contains(text));
    assert!(audited(&format!(
        "request {}: on the card, dropped: notes-agent was retired",
        menu.id
    )));
    assert!(
        !shell.agent_asks.is_open(menu.id),
        "its place on the card is free"
    );
    assert!(
        !shell.agent_admissions.iter().any(|(e, _)| *e == menu.id),
        "its admission is gone"
    );
    // Another question takes the card before the stale answer comes.
    let other = shell.roster_mut().install(ops_manifest()).unwrap();
    let other = u64::from(other.badge());
    assert!(step(&mut shell, other, RR).starts_with("ok asked "));
    let next = take_agents_card(&mut shell);
    assert_ne!(next.id, menu.id);
    let once = host.mint(menu.id, menu.call());
    let reply = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert_eq!(reply, super::ok(crate::agentconsent::RETIRED));
    assert!(audited(&format!(
        "request {}: answered after its agent was retired",
        menu.id
    )));
    assert_eq!(host.policy.get(), None);
    assert_eq!(shell.pending.as_ref().map(|p| p.id), Some(next.id));
    // Said once: a second stale answer finds nothing.
    let again = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(!again.ok, "{again:?}");
    // The question on the card now answers as ever.
    let once = host.mint(next.id, next.call());
    let ran = shell.eval_console(&format!("answer {} allow once={once}", next.id));
    assert!(ran.ok, "{ran:?}");
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

/// An agent retired while its question is on the card and another agent's
/// waits behind it: the person's yes to the stale card runs nothing and logs
/// nothing as anyone's, and the waiting question is the card's next, which
/// runs as its own agent's.
#[test]
fn retiring_the_carded_agent_brings_the_waiting_question_up_next() {
    let (host, mut shell, badge) = with_agent();
    let other = shell.roster_mut().install(ops_manifest()).unwrap();
    let other = u64::from(other.badge());
    assert!(step(&mut shell, badge, RR).starts_with("ok asked "));
    assert!(step(&mut shell, other, RR).starts_with("ok asked "));
    let menu = take_agents_card(&mut shell);
    assert!(
        menu.question().starts_with("notes-agent asks: "),
        "{menu:?}"
    );
    let agent = shell.roster_mut().by_name("notes-agent").unwrap().id();
    shell.retire_agent(agent).unwrap();
    let logged = shell.log.entries().count();
    let once = host.mint(menu.id, menu.call());
    let reply = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert_eq!(reply, super::ok(crate::agentconsent::RETIRED));
    assert_eq!(host.policy.get(), None, "nothing ran");
    assert_eq!(shell.log.entries().count(), logged, "nothing was logged");
    let dropped = shell.log.get(menu.id).unwrap();
    assert_ne!(dropped.state, crate::actlog::State::Done, "{dropped:?}");
    assert_eq!(dropped.agent.as_str(), "notes-agent");
    let next = take_agents_card(&mut shell);
    assert_ne!(next.id, menu.id);
    assert!(next.question().starts_with("ops-agent asks: "), "{next:?}");
    let once = host.mint(next.id, next.call());
    let ran = shell.eval_console(&format!("answer {} allow once={once}", next.id));
    assert!(ran.ok, "{ran:?}");
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    let entry = shell.log.get(shell.log.last_id()).unwrap();
    assert_eq!(entry.agent.as_str(), "ops-agent", "{entry:?}");
}

/// #1478's review: an Allow whose ONCE is missing, or does not check, runs
/// nothing. The agent's wait hears that it did not run, never "ran", so the
/// goals agent does not bill it as a step that ran.
#[test]
fn an_allow_whose_once_is_missing_or_wrong_is_no_run_for_the_agent() {
    for once in ["", " once=0000"] {
        let (host, mut shell, badge) = with_agent();
        let asked = step(&mut shell, badge, RR);
        assert!(asked.starts_with("ok asked "), "{asked}");
        let menu = take_agents_card(&mut shell);
        let answered = shell.eval_console(&format!("answer {} allow{once}", menu.id));
        assert!(answered.text.contains("not run"), "{once:?}: {answered:?}");
        assert_eq!(host.policy.get(), None, "{once:?}: nothing ran");
        let told = step(&mut shell, badge, &format!("wait {}", menu.id));
        assert!(!told.starts_with("ok ran"), "{once:?}: {told}");
        assert!(told.contains("not run"), "{once:?}: {told}");
    }
}
