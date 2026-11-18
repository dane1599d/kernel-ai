//! Agents' jobs: the protocol, who may start and take what, and the scheduler's
//! refusals in the ADR's words.

use super::*;
use crate::{Budgets, Config};
use std::string::ToString;
use std::vec::Vec;

const GOALS: u64 = 2;
const INDEX: u64 = 3;
const NOW: Now = Now { ms: 0, day: 1 };

fn jobs() -> Jobs {
    Jobs::new(Config::default(), 7)
}

fn start(jobs: &mut Jobs, badge: u64, text: &str) -> String {
    let command = parse(text).unwrap().unwrap();
    jobs.handle(badge, command, 40, 20, NOW)
}

fn ticket(reply: &str) -> u64 {
    reply.strip_prefix("ok ").unwrap().parse().unwrap()
}

#[test]
fn the_commands_parse_and_anything_else_is_no_agent_command() {
    assert_eq!(
        parse("agent-start act tidy the notes folder"),
        Ok(Some(Command::Start {
            kind: Kind::Act,
            request: "tidy the notes folder"
        }))
    );
    assert_eq!(parse("agent-take 12"), Ok(Some(Command::Take(12))));
    assert_eq!(parse("agent-stop 9"), Ok(Some(Command::Stop(9))));
    for other in [
        "act switch to round robin",
        "warm",
        "",
        "agent-started x",
        "agent",
    ] {
        assert_eq!(parse(other), Ok(None), "{other:?}");
    }
    for bad in [
        "agent-start",
        "agent-start act",
        "agent-start act   ",
        "agent-start tune go",
        "agent-start ACT go",
        "agent-take",
        "agent-take +1",
        "agent-take 1x",
        "agent-take -1",
        "agent-take 18446744073709551616",
        "agent-stop  1",
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn act_and_script_are_grammar_shaped_and_ask_is_free() {
    assert_eq!(Kind::Act.shape(), Shape::Grammar);
    assert_eq!(Kind::Script.shape(), Shape::Grammar);
    assert_eq!(Kind::Ask.shape(), Shape::Free);
}

#[test]
fn the_person_and_services_start_no_job() {
    let mut jobs = jobs();
    for badge in [NO_AGENT, KAI] {
        let reply = start(&mut jobs, badge, "agent-start ask hello");
        assert!(reply.starts_with("err "), "{badge}: {reply}");
    }
    assert_eq!(jobs.scheduler().pending(), 0);
}

#[test]
fn a_job_waits_then_its_answer_is_taken_once() {
    let mut jobs = jobs();
    let t = ticket(&start(
        &mut jobs,
        GOALS,
        "agent-start script count the files",
    ));
    assert_eq!(jobs.handle(GOALS, Command::Take(t), 0, 0, NOW), "pending");
    let request = jobs.job(t).unwrap().queued.unwrap();
    assert_eq!(jobs.ticket_of(request), Some(t));
    jobs.finish(request, "ok the answer".to_string());
    assert_eq!(jobs.scheduler().pending(), 0);
    assert_eq!(
        jobs.handle(GOALS, Command::Take(t), 0, 0, NOW),
        "ok the answer"
    );
    assert_eq!(
        jobs.handle(GOALS, Command::Take(t), 0, 0, NOW),
        "ended unknown ticket"
    );
}

#[test]
fn a_ticket_is_only_its_own_agents() {
    let mut jobs = jobs();
    let t = ticket(&start(&mut jobs, GOALS, "agent-start ask what is due"));
    // Another agent's take reads as a ticket that never was, and its stop does nothing.
    assert_eq!(
        jobs.handle(INDEX, Command::Take(t), 0, 0, NOW),
        jobs.handle(INDEX, Command::Take(t + 1000), 0, 0, NOW)
    );
    assert_eq!(jobs.handle(INDEX, Command::Stop(t), 0, 0, NOW), "ok");
    assert!(jobs.job(t).is_some());
    assert_eq!(jobs.scheduler().pending(), 1);
    // Its own stop drops it from the scheduler too.
    assert_eq!(jobs.handle(GOALS, Command::Stop(t), 0, 0, NOW), "ok");
    assert!(jobs.job(t).is_none());
    assert_eq!(jobs.scheduler().pending(), 0);
}

#[test]
fn tickets_of_another_run_of_llm_are_unknown() {
    let mut first = Jobs::new(Config::default(), 1);
    let mut second = Jobs::new(Config::default(), 2);
    let a = ticket(&start(&mut first, GOALS, "agent-start ask a"));
    let b = ticket(&start(&mut second, GOALS, "agent-start ask b"));
    assert_ne!(a, b);
    assert_eq!(
        second.handle(GOALS, Command::Take(a), 0, 0, NOW),
        "ended unknown ticket"
    );
}

#[test]
fn the_schedulers_refusals_are_in_the_adrs_words() {
    let mut known = jobs();
    // An agent llm does not know.
    let reply = start(&mut known, 99, "agent-start ask hello");
    assert_eq!(reply, "err this agent is not an agent llm knows");
    // Too long for the window.
    let command = parse("agent-start ask a long one").unwrap().unwrap();
    let reply = known.handle(GOALS, command, 3_000, 2_000, NOW);
    assert!(
        reply.starts_with("err the request from goals needs 5000 positions"),
        "{reply}"
    );
    // A packaged agent over its day.
    assert_eq!(
        refusal_words(
            Refusal::OverQuota {
                used: 100_000,
                budget: 100_000
            },
            "notes-agent"
        ),
        "Over quota: notes-agent used 100000 tokens today"
    );
    let mut over = jobs();
    let budgets = Budgets {
        tokens_per_day: 30,
        kv_positions: 2_048,
        pool_mib: 64,
    };
    over.scheduler().set_agent(Agent {
        id: AgentId(GOALS as u32),
        class: Class::Background,
        weight: 1,
        budgets: Some(budgets),
    });
    let t = ticket(&start(&mut over, GOALS, "agent-start ask first"));
    // Its prompt takes the day's 30 tokens.
    while over.job(t).unwrap().queued.is_some() {
        match over.scheduler().next(NOW) {
            crate::Decision::End { request, .. } => over.ended(request, "over quota"),
            crate::Decision::Idle => break,
            _ => {}
        }
    }
    assert_eq!(
        over.handle(GOALS, Command::Take(t), 0, 0, NOW),
        "ended over quota"
    );
    let reply = start(&mut over, GOALS, "agent-start ask second");
    assert_eq!(reply, "err Over quota: goals used 30 tokens today");
}

#[test]
fn an_agent_keeps_at_most_so_many_jobs() {
    let mut jobs = jobs();
    let tickets: Vec<u64> = (0..MAX_JOBS)
        .map(|_| ticket(&start(&mut jobs, GOALS, "agent-start ask one more")))
        .collect();
    let reply = start(&mut jobs, GOALS, "agent-start ask one too many");
    assert_eq!(
        reply,
        format!("err goals has {MAX_QUEUED} requests waiting")
    );
    // Another agent is not held by it.
    assert!(start(&mut jobs, INDEX, "agent-start ask mine").starts_with("ok "));
    // A done answer not yet taken still counts; once taken, there is room.
    let request = jobs.job(tickets[0]).unwrap().queued.unwrap();
    jobs.finish(request, "ok done".to_string());
    assert!(start(&mut jobs, GOALS, "agent-start ask again").starts_with("err "));
    jobs.handle(GOALS, Command::Take(tickets[0]), 0, 0, NOW);
    assert!(start(&mut jobs, GOALS, "agent-start ask again").starts_with("ok "));
}

#[test]
fn the_built_ins_are_agentids_in_badge_order() {
    use agentid::Builtin;
    let others: Vec<(u64, &str)> = Builtin::ALL[1..]
        .iter()
        .map(|b| (*b as u64 + 1, b.name()))
        .collect();
    let ours: Vec<(u64, &str)> = BUILTIN.iter().map(|(b, n, _)| (*b, *n)).collect();
    assert_eq!(ours, others);
    assert_eq!(Builtin::Kai as u64 + 1, KAI);
}

#[test]
fn the_next_job_is_the_highest_class_then_the_oldest() {
    let mut jobs = jobs();
    let index = ticket(&start(&mut jobs, INDEX, "agent-start ask embed this"));
    let first = ticket(&start(&mut jobs, GOALS, "agent-start script one"));
    let second = ticket(&start(&mut jobs, GOALS, "agent-start script two"));
    assert_eq!(jobs.next_job().map(|(t, _)| t), Some(first));
    let request = jobs.job(first).unwrap().queued.unwrap();
    jobs.finish(request, "ok".to_string());
    assert_eq!(jobs.next_job().map(|(t, _)| t), Some(second));
    let request = jobs.job(second).unwrap().queued.unwrap();
    jobs.finish(request, "ok".to_string());
    // Embedding work only when no background job waits.
    assert_eq!(jobs.next_job().map(|(t, _)| t), Some(index));
    jobs.handle(INDEX, Command::Stop(index), 0, 0, NOW);
    assert!(jobs.next_job().is_none());
}

#[test]
fn a_job_run_whole_is_charged_to_its_agent_and_never_past_its_day() {
    let mut jobs = jobs();
    jobs.scheduler().set_agent(Agent {
        id: AgentId(GOALS as u32),
        class: Class::Background,
        weight: 1,
        budgets: Some(Budgets {
            tokens_per_day: 100,
            kv_positions: 2_048,
            pool_mib: 64,
        }),
    });
    let t = ticket(&start(&mut jobs, GOALS, "agent-start ask what is due"));
    jobs.charge(t, 70, NOW);
    assert_eq!(jobs.scheduler().used_today(AgentId(GOALS as u32)), Some(70));
    jobs.charge(t, 70, NOW);
    assert_eq!(
        jobs.scheduler().used_today(AgentId(GOALS as u32)),
        Some(100)
    );
    let reply = start(&mut jobs, GOALS, "agent-start ask another");
    assert_eq!(reply, "err Over quota: goals used 100 tokens today");
    // A new day starts the count again.
    let tomorrow = Now { ms: 0, day: 2 };
    jobs.charge(t, 5, tomorrow);
    assert_eq!(jobs.scheduler().used_today(AgentId(GOALS as u32)), Some(5));
}
