//! Agents' jobs: the protocol, who may start and take what, and the scheduler's
//! refusals in the ADR's words.

use super::*;
use crate::{Budgets, Config};
use std::string::ToString;
use std::vec::Vec;

const GOALS: u64 = 2;
const INDEX: u64 = 3;
const NOW: Now = Now { ms: 0, day: 1 };

/// A caller on the route with `badge`, from a process whose pid is not it.
fn by(badge: u64) -> Caller {
    Caller {
        pid: 1_000 + badge,
        badge,
    }
}

fn jobs() -> Jobs {
    Jobs::new(Config::default(), 7)
}

fn start(jobs: &mut Jobs, badge: u64, text: &str) -> String {
    let command = parse(text).unwrap().unwrap();
    jobs.handle(by(badge), command, 40, 20, NOW)
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
    assert_eq!(
        jobs.handle(by(GOALS), Command::Take(t), 0, 0, NOW),
        "pending"
    );
    let request = jobs.job(t).unwrap().queued.unwrap();
    assert_eq!(jobs.ticket_of(request), Some(t));
    jobs.finish(request, "ok the answer".to_string());
    assert_eq!(jobs.scheduler().pending(), 0);
    assert_eq!(
        jobs.handle(by(GOALS), Command::Take(t), 0, 0, NOW),
        "ok the answer"
    );
    assert_eq!(
        jobs.handle(by(GOALS), Command::Take(t), 0, 0, NOW),
        "ended unknown ticket"
    );
}

#[test]
fn a_ticket_is_only_its_own_agents() {
    let mut jobs = jobs();
    let t = ticket(&start(&mut jobs, GOALS, "agent-start ask what is due"));
    // Another agent's take reads as a ticket that never was, and its stop does nothing.
    assert_eq!(
        jobs.handle(by(INDEX), Command::Take(t), 0, 0, NOW),
        jobs.handle(by(INDEX), Command::Take(t + 1000), 0, 0, NOW)
    );
    assert_eq!(jobs.handle(by(INDEX), Command::Stop(t), 0, 0, NOW), "ok");
    assert!(jobs.job(t).is_some());
    assert_eq!(jobs.scheduler().pending(), 1);
    // Its own stop drops it from the scheduler too.
    assert_eq!(jobs.handle(by(GOALS), Command::Stop(t), 0, 0, NOW), "ok");
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
        second.handle(by(GOALS), Command::Take(a), 0, 0, NOW),
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
    let reply = known.handle(by(GOALS), command, 3_000, 2_000, NOW);
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
        over.handle(by(GOALS), Command::Take(t), 0, 0, NOW),
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
    jobs.handle(by(GOALS), Command::Take(tickets[0]), 0, 0, NOW);
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
    jobs.handle(by(INDEX), Command::Stop(index), 0, 0, NOW);
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

/// The manager's HIGH on #1008: the kernel's pid is no identity. A process whose pid
/// is a built-in agent's badge number, on a route with no badge, starts nothing and
/// takes nothing; the badge alone names the agent, whatever the pid.
#[test]
fn a_pid_that_equals_a_badge_number_is_no_agent() {
    let mut jobs = jobs();
    let command = parse("agent-start ask am I goals").unwrap().unwrap();
    for pid in [1, 2, 3, 4] {
        let reply = jobs.handle(
            Caller {
                pid,
                badge: NO_AGENT,
            },
            command,
            10,
            10,
            NOW,
        );
        assert!(reply.starts_with("err "), "pid {pid}: {reply}");
    }
    assert_eq!(jobs.scheduler().pending(), 0);
    // goals' job, by its badge, from any pid; the take from pid 2 with no badge is
    // a ticket that never was.
    let t = ticket(&jobs.handle(
        Caller {
            pid: 77,
            badge: GOALS,
        },
        command,
        10,
        10,
        NOW,
    ));
    let pid_two = Caller {
        pid: GOALS,
        badge: NO_AGENT,
    };
    assert_eq!(
        jobs.handle(pid_two, Command::Take(t), 0, 0, NOW),
        "ended unknown ticket"
    );
    assert_eq!(jobs.handle(pid_two, Command::Stop(t), 0, 0, NOW), "ok");
    assert!(jobs.job(t).is_some());
    let other_pid = Caller {
        pid: 9,
        badge: GOALS,
    };
    assert_eq!(
        jobs.handle(other_pid, Command::Take(t), 0, 0, NOW),
        "pending"
    );
}

/// The manager's audit: only the person (and badge 0's services) get the person's
/// commands. An agent's badge may send `agent-*` alone, so it can neither read nor
/// change the person's conversation: llm refuses before it reads the request.
#[test]
fn only_the_person_gets_the_persons_commands() {
    let persons = [
        "act 120000 5 0 0\nhello",
        "remember 1 2\nab",
        "new",
        "app-start x",
        "app-take 1",
        "app-stop 1",
        "explain facts",
        "query q",
        "tune t",
        "warm-lang c",
        "ask what is up",
        "script x",
        "health",
        "warm",
    ];
    for badge in [GOALS, INDEX, 4, 99] {
        for command in persons {
            assert!(!may_serve(badge, command), "badge {badge}: {command:?}");
        }
        assert!(may_serve(badge, "agent-start ask hi"));
        assert!(may_serve(badge, "agent-take 1"));
    }
    for badge in [NO_AGENT, KAI] {
        for command in persons {
            assert!(may_serve(badge, command), "badge {badge}: {command:?}");
        }
    }
}

/// Bug hunt #19: a running job stops only for the person and for its own
/// agent's `agent-stop`; every poll that came while it ran stopped it before.
#[test]
fn a_running_job_stops_only_for_the_person_or_its_own_stop() {
    let running = (7 << 32) | 1;
    let answer = |badge: u64, text: &str| while_job(badge, text, running, GOALS);
    // The planner's own poll, every runner turn, and its other commands.
    assert_eq!(
        answer(GOALS, &format!("agent-take {running}")),
        WhileJob::Answer
    );
    assert_eq!(answer(GOALS, "agent-take 12"), WhileJob::Answer);
    assert_eq!(answer(GOALS, "agent-stop 12"), WhileJob::Answer);
    assert_eq!(answer(GOALS, "agent-start act {}"), WhileJob::Answer);
    assert_eq!(
        answer(INDEX, &format!("agent-stop {running}")),
        WhileJob::Answer
    );
    assert_eq!(answer(GOALS, "agent-take x"), WhileJob::Answer);
    // The Monitor's health, the panel's warm, and what an agent may not send.
    assert_eq!(answer(NO_AGENT, "health"), WhileJob::Answer);
    assert_eq!(answer(KAI, "warm"), WhileJob::Answer);
    assert_eq!(answer(GOALS, "act {}"), WhileJob::Answer);
    // Its own stop, and the person's requests, stop it.
    assert_eq!(
        answer(GOALS, &format!("agent-stop {running}")),
        WhileJob::Stop
    );
    assert_eq!(answer(KAI, "act {}"), WhileJob::Stop);
    assert_eq!(answer(KAI, "ask why"), WhileJob::Stop);
    assert_eq!(answer(NO_AGENT, "model"), WhileJob::Stop);
}
