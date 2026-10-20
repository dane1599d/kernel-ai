//! `tune` on a host that plays the kernel and the llm service: mix A's
//! processes, a scheduler window, canned answers and a trial that ends as told.

use super::*;
use kabi::sched::{SchedStats, ThreadRecord, TrialRequest, reason, state};
use llmproto::chat::Completion;
use llmproto::tune::{CARD_MAX_COLUMNS, CARD_MAX_LINES, TuneRequest};

const NAMES: [&str; 11] = [
    "shell", "llm", "editor", "build-1", "build-2", "build-3", "build-4", "indexer", "hog-1",
    "hog-2", "notes",
];

/// How the trial the host runs ends.
#[derive(Clone, Copy)]
enum Ending {
    Kept,
    Reverted(u64, u64),
}

struct TuneHost {
    clock: Cell<u64>,
    answer: RefCell<String>,
    asked: RefCell<Vec<TuneRequest>>,
    trials: RefCell<Vec<TrialRequest>>,
    ending: Cell<Ending>,
    /// Polls of a running trial so far.
    polls: Cell<u64>,
    spec: Cell<(u64, u64)>,
    audit: RefCell<Vec<String>>,
}

fn pid(name: &str) -> u64 {
    NAMES.iter().position(|n| *n == name).unwrap() as u64 + 10
}

impl TuneHost {
    fn new(answer: &str, ending: Ending) -> Rc<Self> {
        Rc::new(Self {
            clock: Cell::new(1_000_000_000_000),
            answer: RefCell::new(String::from(answer)),
            asked: RefCell::new(Vec::new()),
            trials: RefCell::new(Vec::new()),
            ending: Cell::new(ending),
            polls: Cell::new(0),
            spec: Cell::new((policy::INTERACTIVE, 0)),
            audit: RefCell::new(Vec::new()),
        })
    }
}

impl Host for TuneHost {
    fn now_ns(&self) -> u64 {
        self.clock.get()
    }
    fn interrupted(&self) -> bool {
        false
    }
    fn heap_bytes(&self) -> u64 {
        0
    }
    fn metrics(&self) -> Result<Metrics, kabi::Error> {
        let mut m = Metrics::empty();
        m.cpus = 1;
        for (i, name) in NAMES.iter().enumerate() {
            m.processes[i].pid = pid(name);
            m.processes[i].name[..name.len()].copy_from_slice(name.as_bytes());
            m.processes[i].name_len = name.len() as u64;
        }
        m.process_count = NAMES.len() as u64;
        Ok(m)
    }
    fn set_policy(&self, id: u64) -> Result<(), kabi::Error> {
        self.spec.set((id, 0));
        Ok(())
    }
    fn set_spec(&self, id: u64, preset: u64) -> Result<(), kabi::Error> {
        self.spec.set((id, preset));
        Ok(())
    }
    fn ask(&self, _: &str) -> Result<String, String> {
        Err(String::from("no"))
    }
    fn script(&self, _: &ScriptRequest) -> Result<Answer, AnswerError> {
        Err(AnswerError::Unavailable(String::from("no")))
    }
    fn audit(&self, text: &str) {
        self.audit.borrow_mut().push(String::from(text));
    }
    fn sleep_ms(&self, ms: u64) {
        self.clock.set(self.clock.get() + ms * 1_000_000);
    }
    fn tune(&self, request: &TuneRequest) -> Result<Completion, String> {
        self.asked.borrow_mut().push(request.clone());
        Ok(Completion {
            text: self.answer.borrow().clone(),
            cut_off: false,
        })
    }
    fn sched_trial(&self, request: &TrialRequest) -> Result<u64, kabi::Error> {
        self.trials.borrow_mut().push(*request);
        self.polls.set(0);
        Ok(self.trials.borrow().len() as u64)
    }
    fn sched_stats(&self, window_ms: u64) -> Result<Box<SchedStats>, kabi::Error> {
        let mut s = Box::new(SchedStats::empty());
        s.cpus = 1;
        (s.policy, s.preset) = self.spec.get();
        s.now_ns = self.clock.get();
        if window_ms > 0 {
            s.window_ns = window_ms * 1_000_000;
            s.busy_ns = s.window_ns;
            // The editor waits 6 ms at p99 behind builds that wake 250 times a second.
            for (i, name) in NAMES.iter().enumerate() {
                let build = name.starts_with("build");
                let p99 = if *name == "editor" {
                    6_000_000
                } else {
                    1_000_000
                };
                s.threads[i] = ThreadRecord {
                    tid: i as u64 + 2,
                    pid: pid(name),
                    cpu_ns: if build { 400_000_000 } else { 50_000_000 },
                    wakes: if build { 750 } else { 60 },
                    p50_ns: p99 / 4,
                    p99_ns: p99,
                    max_ns: p99 * 2,
                    samples: 60,
                    ..ThreadRecord::default()
                };
            }
            s.count = NAMES.len() as u64;
        }
        let trials = self.trials.borrow().len() as u64;
        if trials > 0 && window_ms == 0 {
            let polls = self.polls.get() + 1;
            self.polls.set(polls);
            let t = &mut s.trial;
            t.id = trials;
            t.windows_total = 6;
            t.windows_done = (polls / 2).min(6);
            for (i, w) in t.windows.iter_mut().enumerate().take(6) {
                w.candidate = (i % 2) as u64;
                w.target_p99_ns = if w.candidate == 1 { 400_000 } else { 6_100_000 };
                w.background_cpu_ns = 2_000_000_000;
            }
            t.state = state::RUNNING;
            if t.windows_done == 6 {
                t.baseline_p99_ns = 6_100_000;
                t.candidate_p99_ns = 400_000;
                t.background_permille = 970;
                (t.state, t.reason, t.reason_value) = match self.ending.get() {
                    Ending::Kept => (state::KEPT, 0, 0),
                    Ending::Reverted(code, value) => (state::REVERTED, code, value),
                };
            }
        }
        Ok(s)
    }
}

fn answer(policy: &str, preset: &str, background: &[&str]) -> String {
    let names: Vec<String> = background.iter().map(|n| format!("\"{n}\"")).collect();
    format!(
        r#"{{"reason":"The build jobs wake often and delay the editor.","action":"try","policy":"{policy}","preset":"{preset}","background":[{}]}}"#,
        names.join(",")
    )
}

fn fits_one_screen(text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() <= CARD_MAX_LINES, "{text}");
    assert!(lines.iter().all(|l| l.len() <= CARD_MAX_COLUMNS), "{text}");
}

const BUILDS: [&str; 4] = ["build-1", "build-2", "build-3", "build-4"];

#[test]
fn a_good_pick_is_proposed_tried_after_yes_and_kept() {
    let host = TuneHost::new(&answer("classes", "firm", &BUILDS), Ending::Kept);
    let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
    let card = shell.eval("tune editor: make typing snappier");
    assert_eq!(card.confirm, Some(1), "{card:?}");
    fits_one_screen(&card.text);
    assert!(
        card.text
            .contains("The model says (its claim, not checked): The build jobs")
    );
    assert!(card.text.contains("Plan: try classes/firm"));
    let asked = host.asked.borrow()[0].clone();
    assert!(asked.text.contains("make typing snappier"));
    // The protected services are never offered for a label, nor the target.
    for name in ["shell", "llm", "editor"] {
        assert!(!asked.labelable.iter().any(|n| n == name), "{asked:?}");
    }
    let verdict = shell.eval("answer 1 yes");
    assert!(
        verdict.ok && verdict.text.starts_with("Kept classes/firm"),
        "{verdict:?}"
    );
    fits_one_screen(&verdict.text);
    let trial = host.trials.borrow()[0];
    assert_eq!((trial.policy, trial.preset), (policy::CLASSES, 1));
    assert_eq!(trial.target_pid, pid("editor"));
    let labelled = &trial.background[..trial.background_count as usize];
    assert_eq!(labelled, BUILDS.map(pid));
    let audit = host.audit.borrow();
    assert!(
        audit
            .iter()
            .any(|l| l == "window 2/6 candidate: \"editor\" p99 0.4 ms, background CPU 100%"),
        "{audit:?}"
    );
    assert_eq!(audit.iter().filter(|l| l.starts_with("window ")).count(), 6);
    drop(audit);
    // Undo puts back what ran before.
    host.spec.set((policy::CLASSES, 1));
    assert_eq!(shell.eval("tune undo").text, "back to interactive");
    assert_eq!(host.spec.get(), (policy::INTERACTIVE, 0));
    assert!(!shell.eval("tune undo").ok);
}

#[test]
fn an_invalid_answer_keeps_the_current_policy_and_starts_nothing() {
    for bad in [
        String::from("I think classes"),
        answer("classes", "firm", &["llm"]),
        answer("classes", "firm", &["editor"]),
        answer("round-robin", "firm", &["build-1"]),
    ] {
        let host = TuneHost::new(&bad, Ending::Kept);
        let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
        let reply = shell.eval("tune editor");
        assert!(reply.ok && reply.confirm.is_none(), "{reply:?}");
        assert!(reply.text.contains("Kept the current policy"), "{reply:?}");
        assert_eq!(shell.eval("answer 1 yes").text, "nothing to confirm");
        assert!(host.trials.borrow().is_empty());
    }
}

#[test]
fn a_rolled_back_choice_is_not_proposed_again_for_ten_minutes() {
    let host = TuneHost::new(
        &answer("round-robin", "gentle", &[]),
        Ending::Reverted(reason::NOT_FASTER, 0),
    );
    let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
    assert_eq!(shell.eval("tune editor").confirm, Some(1));
    let verdict = shell.eval("answer 1 yes");
    assert!(
        verdict.text.starts_with("Rolled back round-robin"),
        "{verdict:?}"
    );
    let again = shell.eval("tune editor");
    assert!(
        again.confirm.is_none() && again.text.contains("Not proposed"),
        "{again:?}"
    );
    fits_one_screen(&again.text);
    host.clock.set(host.clock.get() + 601_000_000_000);
    assert_eq!(shell.eval("tune editor").confirm, Some(3));
    // Anything but the answer cancels a proposal.
    assert!(shell.eval("1 + 1").ok);
    assert_eq!(shell.eval("answer 3 yes").text, "nothing to confirm");
}

#[test]
fn a_kernel_revert_and_a_no_are_said_plainly() {
    let host = TuneHost::new(
        &answer("classes", "gentle", &["build-1"]),
        Ending::Reverted(reason::STARTER_EXITED, 0),
    );
    let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
    shell.eval("tune editor");
    assert_eq!(shell.eval("answer 1 no").text, "proposal 1 not tried");
    assert!(host.trials.borrow().is_empty());
    shell.eval("tune editor");
    let verdict = shell.eval("answer 2 y");
    assert!(
        verdict
            .text
            .starts_with("Rolled back classes/gentle at once"),
        "{verdict:?}"
    );
    assert!(!shell.eval("tune nobody").ok);
}
