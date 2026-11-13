//! Goals under random sequences (ADR 0013 section 5): what the person approved is all
//! a goal ever does without asking. Goals are made, approved, paused, raised and
//! forgotten, steps are proposed (with paths made to leave a folder, rows outside the
//! scope, and a door that asks for what has no undo or what content steers), answered
//! by the person, finished, charged and cancelled, and the clock runs on. After every
//! step the rules are checked from the goal's own record, with the scope and the
//! budget read again here from their documented words and not from the code under
//! test:
//!
//! - a step runs without a yes only inside the approved rows and folders, when the
//!   door says it runs, within every budget and the deadline, and (when the person is
//!   away) only if the goal runs unattended;
//! - one step at a time, one record per step, and the state agrees with what the goal
//!   holds (a step that waits, a step in flight);
//! - at most `MAX_ACTIVE` goals live and `MAX_GOALS` kept, ids never reused;
//! - what is kept reads back from its own text as the same goals.
//!
//! The generator is splitmix64 on a seed, so a failure names the seed that makes it.

use goalcore::{
    AskKind, Budget, Door, Goal, Goals, MAX_ACTIVE, MAX_GOALS, MAX_LOG, MAX_TARGETS, Pause,
    Presence, Refusal, Scope, Spec, State, Step, StepDone, StepStatus, Tier, Verdict,
};
use std::collections::BTreeSet;

const T0: u64 = 1_000_000;
const MINUTE: u64 = 60_000;
const HOUR: u64 = 60 * MINUTE;
const SEEDS: u64 = 300;
const STEPS: usize = 300;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.next() % (high - low + 1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

/// The one door: a row with no undo asks, and so does a step whose call a note or a
/// page steered (the runner's record of it, here a flag).
struct TestDoor;

impl Door<bool> for TestDoor {
    fn tier(&self, step: &Step<bool>) -> Tier {
        match step.call || ["fs_trash", "net_post"].contains(&step.row.as_str()) {
            true => Tier::Ask,
            false => Tier::Run,
        }
    }
}

// What a scope and a path are, from the documented words.

/// An exact row, or a group `notes_*`.
fn row_in(rows: &[String], row: &str) -> bool {
    rows.iter().any(|entry| match entry.strip_suffix("_*") {
        Some(group) => row.starts_with(&format!("{group}_")),
        None => entry == row,
    })
}

/// A plain relative path: no empty, `.` or `..` part, no backslash, printable.
fn plain(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && path.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn path_in(folders: &[String], path: &str) -> bool {
    plain(path)
        && folders
            .iter()
            .any(|folder| path == folder || path.starts_with(&format!("{folder}/")))
}

fn row_name(row: &str) -> bool {
    row.len() <= 48
        && row.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && row
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Whether a step is in a form a log can keep.
fn loggable(step: &Step<bool>) -> bool {
    row_name(&step.row)
        && step.targets.len() <= MAX_TARGETS
        && step
            .targets
            .iter()
            .all(|path| !path.is_empty() && path.len() <= 255 && printable(path))
}

fn printable(text: &str) -> bool {
    text.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// Which budget or deadline a step of `est` tokens would run past at `now`.
fn over(goal: &Goal, now: u64, est: u64) -> bool {
    let started = goal.started_ms.unwrap_or(now);
    goal.by_ms.is_some_and(|by| now >= by)
        || now.saturating_sub(started) >= goal.budget.wall_ms
        || goal.steps_run >= goal.budget.steps
        || goal.tokens_spent.saturating_add(est) > goal.budget.tokens
}

// The generators.

const ROWS: [&str; 6] = [
    "fs_list", "fs_read", "fs_move", "fs_trash", "notes_*", "net_*",
];
const FOLDERS: [&str; 4] = ["reports", "archive", "reports/sub", "notes"];
const STEP_ROWS: [&str; 9] = [
    "fs_list",
    "fs_read",
    "fs_move",
    "fs_trash",
    "notes_add",
    "net_get",
    "net_post",
    "Bad Row",
    "",
];
/// Paths, most of them made to be taken for something inside a folder.
const PATHS: [&str; 16] = [
    "reports/a.txt",
    "reports",
    "archive/x/y.txt",
    "reports/sub/deep/z",
    "notes/todo",
    "reports/../secret",
    "../reports/a",
    "/etc/passwd",
    "reports//a",
    "reports/./a",
    "reports\\a",
    "reportsx/a",
    "report",
    "elsewhere/b.txt",
    "reports/a\nb",
    "",
];

fn subset(rng: &mut Rng, items: &[&str], most: usize) -> Vec<String> {
    let mut chosen: Vec<String> = Vec::new();
    for _ in 0..rng.below(most + 1) {
        let item = rng.pick(items).to_string();
        if !chosen.contains(&item) {
            chosen.push(item);
        }
    }
    chosen
}

/// A request, and whether it breaks a rule that `goal_set` states.
fn spec(rng: &mut Rng, now: u64) -> (Spec, bool) {
    let by_ms = rng.chance(60).then(|| now + rng.range(HOUR, 12 * HOUR));
    let mut rows = subset(rng, &ROWS, 4);
    if rows.is_empty() {
        rows.push("fs_list".to_string());
    }
    let mut checkins: Vec<u64> = (0..rng.below(3))
        .map(|_| now + rng.range(MINUTE, 6 * HOUR))
        .collect();
    checkins.sort_unstable();
    checkins.dedup();
    checkins.retain(|time| by_ms.is_none_or(|by| *time <= by));
    let mut spec = Spec {
        text: "tidy the reports folder".to_string(),
        by_ms,
        scope: Scope {
            rows,
            folders: subset(rng, &FOLDERS, 3),
        },
        budget: Budget {
            steps: rng.range(1, 6) as u32,
            tokens: rng.range(200, 3000),
            wall_ms: rng.range(HOUR, 6 * HOUR),
        },
        checkins,
        away_ok: rng.chance(50),
    };
    if !rng.chance(12) {
        return (spec, false);
    }
    match rng.below(10) {
        0 => spec.scope.folders.push("../x".to_string()),
        1 => spec.scope.folders.push("/abs".to_string()),
        2 => spec.scope.folders.push("a//b".to_string()),
        3 => spec.scope.folders.push("a/./b".to_string()),
        4 => spec.scope.rows.push("*".to_string()),
        5 => spec.scope.rows.push("FS_LIST".to_string()),
        6 => spec.budget.steps = 0,
        7 => spec.by_ms = Some(now),
        8 => spec.text = "a\nb".to_string(),
        _ => spec.scope.rows.clear(),
    }
    (spec, true)
}

/// A step for `goal`: half of them made to fit what it was approved for (so that
/// steps do run), the rest from the pools above, most of which do not fit.
fn step(rng: &mut Rng, goal: &Goal) -> Step<bool> {
    if rng.chance(50) {
        let entry = &goal.scope.rows[rng.below(goal.scope.rows.len())];
        let row = match entry.strip_suffix('*') {
            Some(group) => format!("{group}add"),
            None => entry.clone(),
        };
        let folders = &goal.scope.folders;
        let targets = match folders.is_empty() {
            true => Vec::new(),
            false => (0..rng.below(3))
                .map(|_| {
                    format!(
                        "{}/f{}.txt",
                        folders[rng.below(folders.len())],
                        rng.below(5)
                    )
                })
                .collect(),
        };
        return Step {
            row,
            targets,
            tokens_est: rng.range(0, 150),
            call: rng.chance(8),
        };
    }
    Step {
        row: rng.pick(&STEP_ROWS).to_string(),
        targets: (0..rng.below(MAX_TARGETS + 2))
            .map(|_| rng.pick(&PATHS).to_string())
            .collect(),
        tokens_est: rng.range(0, 400),
        call: rng.chance(15),
    }
}

/// What the runs reached, so that a test that stopped reaching a rule's cases would
/// say so.
#[derive(Default)]
struct Tally {
    ran: usize,
    ran_after_a_yes: usize,
    scope: usize,
    tier: usize,
    away: usize,
    over: usize,
    ended_running: usize,
    forgotten: usize,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.ran += other.ran;
        self.ran_after_a_yes += other.ran_after_a_yes;
        self.scope += other.scope;
        self.tier += other.tier;
        self.away += other.away;
        self.over += other.over;
        self.ended_running += other.ended_running;
        self.forgotten += other.forgotten;
    }
}

struct World {
    tally: Tally,
    goals: Goals,
    now: u64,
    /// The steps a person said yes to: `(goal, step number)`.
    approved: BTreeSet<(u64, u32)>,
    /// The tier the door gave each step, by `(goal, step number)`.
    tiers: Vec<((u64, u32), Tier)>,
    /// The most ids made, so none is ever used twice.
    top_id: u64,
}

impl World {
    fn new() -> Self {
        Self {
            tally: Tally::default(),
            goals: Goals::new(),
            now: T0,
            approved: BTreeSet::new(),
            tiers: Vec::new(),
            top_id: 0,
        }
    }

    fn some_goal(&self, rng: &mut Rng) -> Option<u64> {
        let list = self.goals.list();
        (!list.is_empty()).then(|| list[rng.below(list.len())].id)
    }

    fn set(&mut self, rng: &mut Rng) {
        let (spec, breaks_a_rule) = spec(rng, self.now);
        let full = self.goals.list().len() >= MAX_GOALS;
        match (self.goals.set(spec, self.now), breaks_a_rule) {
            (Err(Refusal::BadSpec(_)), true) => {}
            (Err(Refusal::TooManyGoals), false) => assert!(full),
            (Ok(id), false) => {
                assert!(!full && id > self.top_id, "goal {id} after {}", self.top_id);
                self.top_id = id;
            }
            (got, bad) => panic!("goal_set (a spec that breaks a rule: {bad}): {got:?}"),
        }
    }

    fn propose(&mut self, rng: &mut Rng) {
        let Some(id) = self.some_goal(rng) else {
            return;
        };
        let step = step(rng, self.goals.get(id).unwrap());
        let person = match rng.chance(50) {
            true => Presence::Here,
            false => Presence::Away,
        };
        let tier = TestDoor.tier(&step);
        let goal = self.goals.get_mut(id).unwrap();
        let before = goal.clone();
        let got = goal.propose(self.now, step.clone(), person, &TestDoor);
        let n = before.log.len() as u32 + 1;
        match got {
            Err(Refusal::NotActive(state)) => assert_eq!(state, before.state),
            Err(Refusal::StepInFlight) => assert!(before.flight.is_some()),
            Err(Refusal::LogFull) => assert!(before.log.len() >= MAX_LOG),
            Err(Refusal::BadProposal(_)) => assert!(!loggable(&step), "{step:?}"),
            Err(other) => panic!("propose: {other:?}"),
            Ok(verdict) => {
                assert!(before.state == State::Active && loggable(&step));
                self.tiers.push(((id, n), tier));
                let in_scope = row_in(&before.scope.rows, &step.row)
                    && step
                        .targets
                        .iter()
                        .all(|path| path_in(&before.scope.folders, path));
                let late = over(&before, self.now, step.tokens_est);
                match verdict {
                    Verdict::Over(pause) => {
                        assert!(late, "held back with room left");
                        assert_eq!(goal.state, State::Paused(pause));
                        self.tally.over += 1;
                    }
                    Verdict::Ask(ask) => {
                        assert!(!late, "asked past a budget");
                        assert_eq!(goal.state, State::Waiting);
                        // Scope first, then the door's tier, then the person being away.
                        let kind = match (in_scope, tier, person, before.away_ok) {
                            (false, ..) => AskKind::Scope,
                            (true, Tier::Ask, ..) => AskKind::Tier,
                            _ => AskKind::Away,
                        };
                        assert_eq!(ask.kind, kind, "{step:?}");
                        match kind {
                            AskKind::Scope => self.tally.scope += 1,
                            AskKind::Tier => self.tally.tier += 1,
                            _ => self.tally.away += 1,
                        }
                    }
                    Verdict::Run => {
                        assert!(!late, "ran past a budget or the deadline: {step:?}");
                        assert!(in_scope, "ran outside the scope: {step:?}");
                        assert_eq!(tier, Tier::Run, "ran a step the door asks for");
                        assert!(
                            person == Presence::Here || before.away_ok,
                            "ran unattended in a goal that waits"
                        );
                        assert_eq!(goal.flight, Some(n));
                        self.tally.ran += 1;
                    }
                }
            }
        }
    }

    fn answer(&mut self, rng: &mut Rng) {
        // The person mostly answers a goal that waits for them.
        let waiting: Vec<u64> = self
            .goals
            .list()
            .iter()
            .filter(|goal| goal.state == State::Waiting)
            .map(|goal| goal.id)
            .collect();
        let id = match waiting.is_empty() || rng.chance(15) {
            true => self.some_goal(rng),
            false => Some(waiting[rng.below(waiting.len())]),
        };
        let Some(id) = id else {
            return;
        };
        let yes = rng.chance(80);
        let now = self.now;
        let goal = self.goals.get_mut(id).unwrap();
        let before = goal.clone();
        if !yes {
            let _ = goal.deny();
            return;
        }
        let est = before
            .waiting
            .as_ref()
            .and_then(|ask| ask.proposal.as_ref())
            .map(|p| p.tokens_est);
        if let Ok(verdict) = goal.approve(now) {
            let est = est.expect("a step waited");
            match verdict {
                Verdict::Run => {
                    assert!(!over(&before, now, est), "approved past a budget");
                    self.approved.insert((id, goal.flight.unwrap()));
                    self.tally.ran_after_a_yes += 1;
                }
                Verdict::Over(pause) => {
                    assert!(over(&before, now, est));
                    assert_eq!(goal.state, State::Paused(pause));
                }
                Verdict::Ask(_) => panic!("an approval asked again"),
            }
        }
    }

    fn finish(&mut self, rng: &mut Rng) {
        let Some(id) = self.some_goal(rng) else {
            return;
        };
        let done = StepDone {
            entry: rng.chance(80).then(|| rng.range(1, 1000)),
            tokens: rng.range(0, 500),
            failed: rng
                .chance(25)
                .then(|| "it did not work\n\u{e9}".to_string()),
        };
        let _ = self.goals.get_mut(id).unwrap().finish_step(done);
    }

    /// One of the calls on a goal that change where it stands.
    fn on_goal(&mut self, rng: &mut Rng) {
        let Some(id) = self.some_goal(rng) else {
            return;
        };
        let now = self.now;
        let goal = self.goals.get_mut(id).unwrap();
        let in_flight = goal.flight.is_some();
        match rng.below(9) {
            0 => drop(goal.clarify("Which folder holds the notes?")),
            1 => drop(goal.answered()),
            2 => drop(goal.complete(now, "all done")),
            3 => drop(goal.fail(now, "gave up")),
            4 => drop(goal.cancel(now)),
            5 => drop(goal.pause()),
            6 => drop(goal.record_met(rng.chance(50))),
            7 => goal.charge(rng.range(0, 700)),
            _ => goal.plan(rng.range(0, 90) as u32, "move the old ones"),
        }
        // A stop or a give-up is what ends a goal in the middle of a step.
        self.tally.ended_running += usize::from(in_flight && goal.state.is_final());
    }

    /// The calls that go through the set of goals, which hold the limits.
    fn on_goals(&mut self, rng: &mut Rng) {
        let Some(id) = self.some_goal(rng) else {
            return;
        };
        let now = self.now;
        let more = Budget {
            steps: rng.range(0, 4) as u32,
            tokens: rng.range(0, 1500),
            wall_ms: rng.range(0, 2 * HOUR),
        };
        match rng.below(6) {
            0 | 1 => drop(self.goals.approve(id, now)),
            2 => drop(self.goals.resume(id)),
            3 => drop(self.goals.raise(id, now, more)),
            4 => drop(self.goals.extend(id, now, now + rng.range(0, 3 * HOUR))),
            _ => self.tally.forgotten += usize::from(self.goals.forget(id).is_ok()),
        }
    }

    fn tick(&mut self) {
        for (id, line) in self.goals.tick(self.now) {
            assert!(self.goals.get(id).is_some(), "a check-in for goal {id}");
            assert!(!line.line.is_empty() && !line.line.contains('\n'));
        }
    }

    /// What must be true after every step.
    fn check(&self) {
        let list = self.goals.list();
        assert!(list.len() <= MAX_GOALS);
        assert!(
            self.goals.live() <= MAX_ACTIVE,
            "{} live",
            self.goals.live()
        );
        assert!(list.windows(2).all(|pair| pair[0].id < pair[1].id));
        assert!(
            list.iter()
                .all(|goal| goal.id >= 1 && goal.id <= self.top_id)
        );
        for goal in list {
            self.check_goal(goal);
        }
    }

    fn check_goal(&self, goal: &Goal) {
        let id = goal.id;
        let running: Vec<u32> = goal
            .log
            .iter()
            .filter(|step| step.status == StepStatus::Running)
            .map(|step| step.n)
            .collect();
        assert!(goal.log.len() <= MAX_LOG);
        assert!(goal.log.iter().zip(1..).all(|(step, n)| step.n == n));
        assert_eq!(
            goal.state == State::Waiting,
            goal.waiting.is_some(),
            "#{id}"
        );
        // One step at a time, and it is the one the goal says is in flight.
        assert_eq!(running.first().copied(), goal.flight, "#{id}");
        assert!(running.len() <= 1, "#{id} runs {running:?}");
        if goal.flight.is_some() {
            assert!(matches!(
                goal.state,
                State::Active | State::Paused(Pause::User)
            ));
            assert_eq!(goal.log.last().map(|step| step.n), goal.flight);
        }
        if goal.state.is_final() {
            assert!(goal.flight.is_none() && goal.waiting.is_none(), "#{id}");
        }
        if goal.state == State::Proposed {
            assert!(goal.log.is_empty() && goal.started_ms.is_none(), "#{id}");
        }
        // Spent within what was approved, as far as the goal itself counts.
        assert!(
            goal.steps_run <= goal.budget.steps,
            "#{id} ran too many steps"
        );
        assert_eq!(goal.steps_ok + goal.failed, goal.steps_run);
        let denied = goal
            .log
            .iter()
            .filter(|s| s.status == StepStatus::Denied)
            .count();
        assert_eq!(goal.denied as usize, denied);
        // A step that ran without a person's yes was inside the approval, a run-tier
        // step of the door's, in a goal that was allowed to go on while the person was
        // away if they were.
        for step in &goal.log {
            let ran = matches!(
                step.status,
                StepStatus::Running | StepStatus::Ran | StepStatus::Failed
            );
            if !ran || self.approved.contains(&(id, step.n)) {
                continue;
            }
            let p = &step.proposal;
            assert!(row_in(&goal.scope.rows, &p.row), "#{id} ran {}", p.row);
            assert!(
                p.targets
                    .iter()
                    .all(|path| path_in(&goal.scope.folders, path)),
                "#{id} ran {} on {:?} outside {:?}",
                p.row,
                p.targets,
                goal.scope.folders
            );
            assert_eq!(p.tier, Tier::Run, "#{id} ran a step that asks");
            let door = self.tiers.iter().find(|(key, _)| *key == (id, step.n));
            assert_eq!(door.map(|(_, tier)| *tier), Some(Tier::Run));
            assert!(
                goal.started_ms.is_some(),
                "#{id} ran before it was approved"
            );
        }
    }
}

fn run(seed: u64) -> Tally {
    let mut rng = Rng(seed);
    let mut world = World::new();
    for at in 0..STEPS {
        world.now += rng.range(0, 40 * MINUTE);
        match rng.below(100) {
            0..8 => world.set(&mut rng),
            8..20 => world.on_goals(&mut rng),
            20..50 => world.propose(&mut rng),
            50..62 => world.answer(&mut rng),
            62..74 => world.finish(&mut rng),
            74..88 => world.on_goal(&mut rng),
            _ => world.tick(),
        }
        world.check();
        if at % 25 == 24 {
            let text = world.goals.to_text();
            let back = Goals::parse(&text);
            assert_eq!(back.as_ref().ok(), Some(&world.goals), "{back:?}\n{text}");
            assert_eq!(back.unwrap().to_text(), text);
        }
    }
    world.tally
}

#[test]
fn what_the_person_approved_is_all_a_goal_does_without_asking() {
    let mut total = Tally::default();
    for seed in 0..SEEDS {
        // A failure names its seed.
        match std::panic::catch_unwind(|| run(seed)) {
            Ok(tally) => total.add(&tally),
            Err(cause) => {
                eprintln!("seed {seed} failed");
                std::panic::resume_unwind(cause);
            }
        }
    }
    // Every case a rule is about was reached, many times.
    let reached = [
        ("ran", total.ran),
        ("ran after a yes", total.ran_after_a_yes),
        ("asked for scope", total.scope),
        ("asked for tier", total.tier),
        ("asked for being away", total.away),
        ("held back", total.over),
        ("ended in the middle of a step", total.ended_running),
        ("forgotten", total.forgotten),
    ];
    for (what, count) in reached {
        assert!(count >= 20, "only {count} goals {what}");
    }
}

/// A goal that is stopped, or gives up, while a step runs: the step is closed as
/// failed, and the goal's own file reads back.
#[test]
fn a_goal_ended_while_a_step_runs_counts_it_as_failed_and_reads_back() {
    for end in 0..3 {
        let mut goals = Goals::new();
        let spec = Spec {
            text: "tidy the reports folder".to_string(),
            by_ms: None,
            scope: Scope {
                rows: strings(&["fs_list"]),
                folders: strings(&["reports"]),
            },
            budget: Budget {
                steps: 2,
                tokens: 1000,
                wall_ms: HOUR,
            },
            checkins: Vec::new(),
            away_ok: true,
        };
        let id = goals.set(spec, T0).unwrap();
        goals.approve(id, T0 + 1).unwrap();
        let goal = goals.get_mut(id).unwrap();
        let listing = Step {
            row: "fs_list".to_string(),
            targets: strings(&["reports"]),
            tokens_est: 10,
            call: false,
        };
        let verdict = goal.propose(T0 + 2, listing, Presence::Here, &TestDoor);
        assert_eq!(verdict, Ok(Verdict::Run));
        match end {
            0 => goal.cancel(T0 + 3).unwrap(),
            1 => goal.fail(T0 + 3, "gave up").unwrap(),
            _ => {
                goal.pause().unwrap();
                goal.cancel(T0 + 3).unwrap();
            }
        }
        assert_eq!(goal.log[0].status, StepStatus::Failed);
        assert_eq!((goal.steps_run, goal.steps_ok, goal.failed), (1, 0, 1));
        let text = goals.to_text();
        assert_eq!(Goals::parse(&text).as_ref(), Ok(&goals), "{text}");
    }
}
