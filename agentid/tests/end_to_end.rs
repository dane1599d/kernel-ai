//! A `kaiagent` manifest, end to end, with fakes and no OS (ADR 0013 sections 1, 2
//! and 5): it becomes an agent with a badge (`agentid`), its model requests go
//! through the scheduler under its budgets (`agentsched`), and its goal's steps go
//! through the one door under its name (`goalcore`), with the person's yes taken
//! only from the person.
//!
//! The fakes are the engine (the clock moves 10 ms a token), the door (a row's own
//! tier from the registry, as `agentabi::derive` has it) and the person (hardware
//! input, which only this test's stand-in for the input path may claim).

use agentabi::{ROWS, Tier as RowTier};
use agentid::{AnswerError, Asks, Denied, Line, Mark, Roster, Source};
use agentsched::{
    Agent as SchedAgent, AgentId as SchedId, Ask, Class, Config, Decision, Now, Refusal, RequestId,
    Scheduler,
};
use goalcore::{
    AskKind, Budget, Door, Goals, Presence, Scope, Spec, State, Step, StepDone, Tier, Verdict,
};
use kaiagent::Manifest;

/// A background agent that lists, reads and creates files, with 600 tokens a day.
const MANIFEST: &str = "kai-app 1\nid report-bot\nabi 1.0\nagent 1\nclass background\n\
    model tokens/day=600 kv=1024 pool=64m\nactions fs_list fs_read fs_create\n\
    memory none\nnet none\n";

/// A planning request: 250 tokens to read, 50 to write.
const PROMPT: u32 = 250;
const ANSWER: u32 = 50;
const HOUR: u64 = 3_600_000;
const T0: u64 = 1_000_000;

/// The person's own keyboard: this test stands in for the input path.
fn person() -> Source {
    // SAFETY: a test is the input path here.
    unsafe { Source::hardware() }
}

/// The door, as the registry decides a row's tier: undoable and local runs, the rest
/// asks.
struct RowDoor;

impl Door for RowDoor {
    fn tier(&self, step: &Step) -> Tier {
        match agentabi::find(&ROWS, &step.row).map(agentabi::derive) {
            Some(RowTier::Run) => Tier::Run,
            _ => Tier::Ask,
        }
    }
}

/// The model: a scheduler and a clock, and an engine that does what it is told.
struct Model {
    sched: Scheduler,
    now: Now,
}

impl Model {
    fn ask(&mut self, agent: u32, prompt: u32, new: u32) -> Result<RequestId, Refusal> {
        let ask = Ask {
            agent: SchedId(agent),
            prompt_tokens: prompt,
            max_new_tokens: new,
        };
        self.sched.submit(ask, self.now)
    }

    /// Lets the scheduler run one quantum, as the engine would.
    fn one_quantum(&mut self) {
        loop {
            match self.sched.next(self.now) {
                Decision::Move { .. } => {}
                Decision::Run(quanta) => {
                    self.now.ms += quanta.iter().map(|q| 10 * u64::from(q.tokens)).sum::<u64>();
                    return;
                }
                _ => return,
            }
        }
    }

    /// Runs until nothing waits; the requests in the order they finished.
    fn run(&mut self) -> Vec<(RequestId, u32)> {
        let mut finished = Vec::new();
        loop {
            match self.sched.next(self.now) {
                Decision::Idle => return finished,
                Decision::Move { .. } => {}
                Decision::End { request, agent, .. } => finished.push((request, agent.0)),
                Decision::Run(quanta) => {
                    for q in quanta {
                        self.now.ms += 10 * u64::from(q.tokens);
                        if q.last {
                            finished.push((q.request, q.agent.0));
                        }
                    }
                }
            }
        }
    }
}

fn spec() -> Spec {
    let rows = |names: &[&str]| names.iter().map(|n| (*n).to_string()).collect();
    Spec {
        text: String::from("draft the quarterly report from my notes"),
        by_ms: Some(T0 + 8 * HOUR),
        scope: Scope {
            rows: rows(&["fs_list", "fs_read", "fs_create"]),
            folders: rows(&["reports"]),
        },
        budget: Budget {
            steps: 5,
            tokens: 10_000,
            wall_ms: 10 * HOUR,
        },
        checkins: Vec::new(),
        away_ok: true,
    }
}

fn step(row: &str, path: &str) -> Step {
    Step {
        row: row.to_string(),
        targets: vec![path.to_string()],
        tokens_est: 300,
        call: (),
    }
}

#[test]
fn a_manifest_runs_through_admission_the_scheduler_and_a_goal() {
    // The manifest becomes an agent: a badge, a name, and its own grants as the ceiling.
    let manifest = Manifest::parse(MANIFEST).unwrap();
    let mut roster = Roster::new();
    let id = roster.install(manifest.clone()).unwrap();
    let bot = roster.caller(id.badge()).unwrap();
    let kai = roster.caller(1).unwrap();
    assert_eq!(bot.name().as_str(), "report-bot");
    assert_eq!(roster.admit(&bot, "fs_trash"), Err(Denied::OutsideGrants));
    assert_eq!(roster.admit(&bot, "set_policy"), Err(Denied::OutsideGrants));

    // The person's words reach the person's agent and no other.
    let line = Line::new("put old.log in the trash", person());
    let door = roster.admit(&kai, "fs_trash").unwrap();
    assert_eq!(door.consent_words(&line), "put old.log in the trash");
    let own = roster.admit(&bot, "fs_read").unwrap();
    assert_eq!(own.consent_words(&line), "");

    // The scheduler holds the agent to its manifest: a background agent with 600 tokens
    // a day, and the person's own requests first.
    let mut model = Model {
        sched: Scheduler::new(Config::default()),
        now: Now { ms: 0, day: 1 },
    };
    model
        .sched
        .set_agent(SchedAgent::builtin(SchedId(1), Class::Foreground, 1));
    model
        .sched
        .set_agent(SchedAgent::packaged(SchedId(id.badge()), &manifest, 1));

    // The agent's goal: approved by the person once, then run step by step as the agent.
    let mut goals = Goals::new();
    let goal_id = goals.set(spec(), T0).unwrap();
    goals.approve(goal_id, T0).unwrap();
    let mut asks = Asks::new();
    let mut stamped = Vec::new();
    let mut now = T0;

    // Step 1 and 2: read the notes, then write the report. Each plans through the model
    // and acts through the door, stamped with the agent's name.
    for (n, (row, path)) in [("fs_list", "reports"), ("fs_create", "reports/q3.txt")]
        .into_iter()
        .enumerate()
    {
        let planned = model.ask(id.badge(), PROMPT, ANSWER).unwrap();
        if n == 0 {
            // The person types a request while the agent is mid-sentence: it goes first.
            model.one_quantum();
            model.ask(1, 40, 10).unwrap();
        }
        let order = model.run();
        assert!(order.contains(&(planned, id.badge())), "{order:?}");
        if n == 0 {
            assert_eq!(order.first().map(|(_, agent)| *agent), Some(1), "{order:?}");
        }
        let admission = roster.admit(&bot, row).unwrap();
        let goal = goals.get_mut(goal_id).unwrap();
        let verdict = goal.propose(now, step(row, path), Presence::Away, &RowDoor);
        assert_eq!(verdict, Ok(Verdict::Run), "{row}");
        goal.finish_step(StepDone {
            entry: Some(100 + n as u64),
            tokens: u64::from(PROMPT + ANSWER),
            failed: None,
        })
        .unwrap();
        stamped.push(format!("{} {row}", admission.stamp().field()));
        now += 60_000;
    }
    assert_eq!(
        stamped,
        ["agent=report-bot fs_list", "agent=report-bot fs_create"]
    );
    assert_eq!(model.sched.used_today(SchedId(id.badge())), Some(600));

    // A step in the agent's grants but outside the goal's folders asks. The agent cannot
    // answer for the person; the person's yes runs that one step, for that call only.
    let goal = goals.get_mut(goal_id).unwrap();
    let outside = step("fs_read", "private/diary.txt");
    let verdict = goal
        .propose(now, outside, Presence::Away, &RowDoor)
        .unwrap();
    assert!(
        matches!(&verdict, Verdict::Ask(ask) if ask.kind == AskKind::Scope),
        "{verdict:?}"
    );
    assert_eq!(goal.state, State::Waiting);
    let shown = "fs_read(\"private/diary.txt\")";
    let read = roster.admit(&bot, "fs_read").unwrap();
    asks.ask(&roster, 7, &read, shown).unwrap();
    let from_agent = asks.answer(7, true, Mark::of(Source::agent(id)));
    assert_eq!(from_agent, Err(AnswerError::NotThePerson));
    let yes = asks.answer(7, true, Mark::of(person())).unwrap();
    assert!(yes.spend(&roster, &read, 7, shown));
    assert_eq!(goal.approve(now), Ok(Verdict::Run));
    goal.finish_step(StepDone {
        entry: Some(103),
        tokens: 300,
        failed: None,
    })
    .unwrap();

    // The model's budget: 600 tokens spent, so the third planning request is refused
    // until the day turns over.
    let over = model.ask(id.badge(), PROMPT, ANSWER);
    assert_eq!(
        over,
        Err(Refusal::OverQuota {
            used: 600,
            budget: 600
        })
    );
    model.now.day += 1;
    assert!(model.ask(id.badge(), PROMPT, ANSWER).is_ok());

    // The goal is done, and still within what was approved.
    let goal = goals.get_mut(goal_id).unwrap();
    goal.complete(now, "drafted").unwrap();
    assert_eq!(goal.state, State::Done);
    assert!(goal.steps_run == 3 && goal.failed == 0 && goal.asked == 1);
}
