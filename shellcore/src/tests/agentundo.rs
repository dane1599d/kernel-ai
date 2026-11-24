//! `undo <agent>` holds to the bare `undo`'s rule for that agent's entries:
//! it does not reach past an entry of that agent whose outcome is unknown,
//! since that call may have run. Other agents' and the person's entries do
//! not stop it, nor does an unknown outcome older than what it undoes.

use super::act::with_calls;
use super::*;
use crate::actlog::State;
use alloc::vec;

const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn name(agent: &str) -> agentid::Name {
    agentid::Name::new(agent).unwrap()
}

/// The person's round-robin switch, logged as `agent`'s.
fn switched_by(agent: &str) -> (Rc<FakeHost>, Shell) {
    let (host, mut shell) = with_calls(vec![call(RR)]);
    assert!(shell.eval("switch to round robin").ok);
    let id = shell.log.last_id();
    shell.log.get_mut(id).unwrap().agent = name(agent);
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    (host, shell)
}

/// An entry of `agent`'s whose outcome is unknown (`notes_add` may have run),
/// with no change of its own on the policy.
fn unknown_of(shell: &mut Shell, agent: agentid::Name) {
    let mut entry = shell.log.get(shell.log.last_id()).unwrap().clone();
    // A fresh id, as the shell would give it: the undo's entry comes after.
    entry.id = shell.take_id();
    entry.state = State::Unknown;
    entry.outcome = String::from("failed; notes_add may have run");
    entry.changes.clear();
    entry.agent = agent;
    shell.log.push(entry);
}

/// No two entries share an id (the undo's entry comes after the made-up one).
fn ids_unique(shell: &Shell) -> bool {
    let ids: Vec<u64> = shell.log.entries().map(|e| e.id).collect();
    ids.windows(2).all(|w| w[0] < w[1])
}

fn refused_as_unknown(reply: &Reply) -> bool {
    !reply.ok && reply.text.contains("the outcome of notes_add is not known")
}

#[test]
fn an_agents_newer_unknown_outcome_stops_undo_of_that_agent() {
    let (host, mut shell) = switched_by("tidy");
    unknown_of(&mut shell, name("tidy"));
    let reply = shell.eval("undo tidy");
    assert!(refused_as_unknown(&reply), "{reply:?}");
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

#[test]
fn another_agents_unknown_outcome_does_not_stop_it() {
    let (host, mut shell) = switched_by("tidy");
    unknown_of(&mut shell, name("mailer"));
    let reply = shell.eval("undo tidy");
    assert!(reply.ok && reply.text.starts_with("Undone: "), "{reply:?}");
    assert!(ids_unique(&shell));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn the_persons_unknown_outcome_does_not_stop_it() {
    let (host, mut shell) = switched_by("tidy");
    unknown_of(&mut shell, agentid::Name::kai());
    let reply = shell.eval("undo tidy");
    assert!(reply.ok && reply.text.starts_with("Undone: "), "{reply:?}");
    assert!(ids_unique(&shell));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn an_unknown_outcome_older_than_the_live_entry_does_not_stop_it() {
    // `tidy`'s first switch becomes an unknown outcome; its second is live.
    let (host, mut shell) = with_calls(vec![call(RR), call(RR)]);
    assert!(shell.eval("switch to round robin").ok);
    let first = shell.log.get_mut(shell.log.last_id()).unwrap();
    first.state = State::Unknown;
    first.outcome = String::from("failed; notes_add may have run");
    first.changes.clear();
    first.agent = name("tidy");
    host.policy.set(Some(policy::INTERACTIVE));
    assert!(shell.eval("switch to round robin").ok);
    shell.log.get_mut(shell.log.last_id()).unwrap().agent = name("tidy");
    let reply = shell.eval("undo tidy");
    assert!(reply.ok && reply.text.starts_with("Undone: "), "{reply:?}");
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn an_agent_with_only_unknown_outcomes_is_told_so() {
    let (host, mut shell) = switched_by("tidy");
    // The switch is the person's: `tidy` has only the unknown outcome.
    shell.log.get_mut(shell.log.last_id()).unwrap().agent = agentid::Name::kai();
    unknown_of(&mut shell, name("tidy"));
    let reply = shell.eval("undo tidy");
    assert!(refused_as_unknown(&reply), "{reply:?}");
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

/// `memo-agent`, which may set the policy and open files in the editor
/// (`ui_open` forwards to the editor's own row), on a host with the editor,
/// and its route's badge.
fn with_memo_agent() -> (Rc<FakeHost>, Shell, u64) {
    let host = FakeHost::new();
    host.apps
        .borrow_mut()
        .rows
        .load("editor", super::apps::EDITOR)
        .unwrap();
    let mut shell = shell(&host);
    let manifest = kaiagent::Manifest::parse(
        "kai-app 1\nid memo-agent\nabi 1.0\nagent 1\nclass background\n\
         model tokens/day=50000 kv=1024 pool=64m\nactions set_policy ui_open editor_open_file\n\
         memory none\nnet none\n",
    )
    .unwrap();
    let id = shell.roster_mut().install(manifest).unwrap();
    (host, shell, u64::from(id.badge()))
}

/// `memo-agent`'s step `json`, which asks, allowed on its card by the
/// person; the entry's id.
fn allowed_step(host: &FakeHost, shell: &mut Shell, badge: u64, json: &str) -> u64 {
    let asked = super::agentstep::step(shell, badge, json);
    assert!(asked.starts_with("ok asked "), "{asked}");
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).expect("the agent's card");
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert_eq!(shell.log.get(menu.id).unwrap().agent.as_str(), "memo-agent");
    menu.id
}

const OPEN: &str = r#"{"name":"ui_open","arguments":{"app":"editor","arg":"ideas.txt"}}"#;

#[test]
fn an_agents_allowed_call_that_never_answered_stops_undo_of_that_agent() {
    // The reviewer's case: the agent's live switch, then its asked step the
    // person allows, whose call the app takes and never answers.
    let (host, mut shell, badge) = with_memo_agent();
    let switched = allowed_step(&host, &mut shell, badge, RR);
    assert!(shell.log.get(switched).unwrap().live());
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    host.app_errors.borrow_mut().push_back(String::from(
        "unanswered editor took the call and did not answer; it may have run",
    ));
    let opened = allowed_step(&host, &mut shell, badge, OPEN);
    assert_eq!(shell.log.get(opened).unwrap().state, State::Unknown);
    let reply = shell.eval("undo memo-agent");
    assert!(
        !reply.ok
            && reply
                .text
                .contains("the outcome of editor_open_file is not known"),
        "{reply:?}"
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert_eq!(host.app_requests.borrow().len(), 1, "no undo sent");
}

#[test]
fn an_agents_allowed_call_that_succeeded_is_what_undo_of_that_agent_takes_back() {
    // The twin: the allowed call answers, so it is the agent's newest live
    // entry and `undo memo-agent` takes it back, not the older switch.
    let (host, mut shell, badge) = with_memo_agent();
    allowed_step(&host, &mut shell, badge, RR);
    host.app_replies
        .borrow_mut()
        .push_back(String::from("kai-reply 1\nresult ok(\"opened\")\nundo 4\n"));
    let opened = allowed_step(&host, &mut shell, badge, OPEN);
    assert!(shell.log.get(opened).unwrap().live());
    host.app_replies
        .borrow_mut()
        .push_back(String::from("kai-reply 1\nundone 4\nundo 5\n"));
    let reply = shell.eval("undo memo-agent");
    assert!(reply.ok && reply.text.starts_with("Undone: "), "{reply:?}");
    assert_eq!(host.app_requests.borrow()[1].1, "kai-undo 1\nid 4\n");
    assert!(!shell.log.get(opened).unwrap().live(), "the call's entry");
    assert_eq!(
        host.policy.get(),
        Some(policy::ROUND_ROBIN),
        "the switch stays"
    );
}
