//! `agent=` (ADR 0013 section 1): a plan line names the built-in agent it runs,
//! and the kernel puts that agent's badge on its route to `llm`.

use super::*;
use kabi::handle::agent;

fn errors(text: &str) -> Vec<String> {
    parse_plan(text).errors
}

#[test]
fn kabis_badges_are_the_rosters() {
    let roster = agentid::Roster::new();
    for which in agentid::Builtin::ALL {
        let (badge, _) = agent::builtin(which.name()).expect("every built-in is in kabi");
        let id = roster.by_name(which.name()).expect("on the roster").id();
        assert_eq!(u64::from(id.badge()), badge, "{}", which.name());
    }
    assert_eq!(agent::BUILTIN.len(), agentid::Builtin::ALL.len());
}

#[test]
fn every_shell_line_is_the_agent_kai() {
    for line in kabi::plan::SHELL_LINES {
        let text = alloc::format!(
            "llm caps=serve\ndesktop caps=serve\npkg caps=serve\n{}\n{}",
            kabi::plan::CC,
            kabi::plan::with_updated(line, "")
        );
        let plan = parse_plan(&text);
        assert_eq!(plan.errors, Vec::<String>::new(), "{line}");
        let shell = plan.services.iter().find(|s| s.name == "shell").unwrap();
        assert_eq!(shell.agent, Some(agent::KAI), "{line}");
        assert_eq!(
            plan.grants("shell", "shell").unwrap().agent,
            Some(agent::KAI)
        );
        // Its startup handles are as many as before: the badge rides on a route.
        assert!(shell.startup_handles() <= kabi::handle::MAX_STARTUP);
        // No other line is an agent.
        assert!(
            plan.services
                .iter()
                .filter(|s| s.name != "shell")
                .all(|s| s.agent.is_none())
        );
    }
}

#[test]
fn a_built_in_agent_runs_its_own_program_with_a_route_to_llm() {
    let plan = parse_plan("llm caps=serve\ngoals caps=call uses=llm agent=goals");
    assert_eq!(plan.errors, Vec::<String>::new());
    assert_eq!(plan.services[1].agent, Some(agent::GOALS));
    assert_eq!(
        plan.grants("goals", "goals").unwrap().agent,
        Some(agent::GOALS)
    );
    // The grant is for the line's own program, as dir= and netdest= are.
    assert!(plan.grants("goals", "comphost").is_err());
    for (line, error) in [
        (
            "bob caps=call uses=llm agent=bob",
            "bob: agent=bob is not a built-in agent",
        ),
        (
            "kai2 program=shell caps=call uses=llm agent=kai",
            "kai2: agent=kai is for the line shell running shell",
        ),
        (
            "shell program=comphost caps=call uses=llm agent=kai",
            "shell: agent=kai is for the line shell running shell",
        ),
        (
            "maint program=goals caps=call uses=llm agent=maint",
            "maint: agent=maint is for the line maint running maint",
        ),
        (
            "goals caps=call agent=goals",
            "goals: agent= needs a route to llm in uses=",
        ),
        (
            "goals caps=call uses=llm agent=goals agent=goals",
            "goals: agent= given twice",
        ),
        (
            "goals unsupervised agent=goals",
            "goals: an unsupervised process takes no caps=, uses=, dir=, bootfiles=, netdest= or agent=",
        ),
    ] {
        assert_eq!(
            errors(&alloc::format!("llm caps=serve\n{line}")),
            [error],
            "{line}"
        );
    }
}

/// An agent's route to the shell carries its badge, as its route to llm does,
/// and is pinned: the shell admits its steps by that badge alone. No other
/// line may route to the shell, so no other route to it carries a badge.
#[test]
fn an_agents_route_to_the_shell_carries_its_badge_and_is_pinned() {
    use kabi::handle::agent::{GOALS, badge};
    let plan = parse_plan(
        "llm caps=serve
shell caps=serve
goals caps=call uses=llm+shell agent=goals",
    );
    assert_eq!(plan.errors, Vec::<String>::new());
    let goals = plan.grants("goals", "goals").unwrap();
    assert_eq!(badge("shell", goals.agent), GOALS);
    assert_eq!(badge("llm", goals.agent), GOALS);
    assert_eq!(goals.pinned, ["llm", "shell"]);
    // Any other line's route to the shell is refused, as before.
    assert_eq!(
        errors(
            "shell caps=serve
other caps=call uses=shell"
        ),
        ["other: no route may go to shell"]
    );
    // The system plan's lines route to the shell nowhere, badged or not.
    for files in [false, true] {
        let plan = parse_plan(&kabi::plan::system::plan("llm caps=serve", files));
        for service in &plan.services {
            assert!(
                !service.uses.iter().any(|used| used == "shell"),
                "{}",
                service.name
            );
        }
    }
    // Only the badged services' routes carry it.
    assert_eq!(badge("fetch", Some(GOALS)), 0);
}

/// A route the process may call but never pass on: the agent's route to
/// `llm`, whose badge is its identity, and a route to the send role.
#[test]
fn the_shells_badged_and_sending_routes_are_pinned() {
    for files in [false, true] {
        let plan = parse_plan(&kabi::plan::system::plan("llm caps=serve", files));
        assert_eq!(plan.errors, Vec::<String>::new());
        let shell = plan.grants("shell", "shell").unwrap();
        assert_eq!(shell.pinned, ["llm", "send"]);
        for service in plan.services.iter().filter(|s| s.name != "shell") {
            let grants = plan.grants(&service.name, &service.program).unwrap();
            assert!(grants.pinned.is_empty(), "{}", service.name);
        }
    }
    // A route to llm is pinned only for an agent.
    let plan = parse_plan("llm caps=serve\nupdated caps=call uses=llm");
    assert!(plan.grants("updated", "updated").unwrap().pinned.is_empty());
}
