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
    entry.id = shell.log.last_id() + 1;
    entry.state = State::Unknown;
    entry.outcome = String::from("failed; notes_add may have run");
    entry.changes.clear();
    entry.agent = agent;
    shell.log.push(entry);
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
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn the_persons_unknown_outcome_does_not_stop_it() {
    let (host, mut shell) = switched_by("tidy");
    unknown_of(&mut shell, agentid::Name::kai());
    let reply = shell.eval("undo tidy");
    assert!(reply.ok && reply.text.starts_with("Undone: "), "{reply:?}");
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
