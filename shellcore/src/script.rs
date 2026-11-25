//! LLM-written scripts: checked in a strict profile, analysed, tried against stub
//! functions, and run in a fresh engine that has only the functions the script
//! calls: at once if consent and the tiers let every call run, after a yes if one
//! asks. Each run is one transaction (ADR 0002 D5): the first change to each
//! resource records the value it replaced, every live call is re-checked against
//! the analysed plan, a failure puts every change back, and outbound effects wait
//! in an outbox until the run succeeds.
//!
//! The strict profile has no closures, no `fn`, no indirect calls (`Fn`, `call`,
//! `curry`, `eval`) and no undefined variables, so a script can only reach an OS
//! function by writing a call of it. Its only loop is `for x in <array, string or
//! range>`, which ends by itself; `while`, `loop` and `do` stay off, and the
//! operation, time and heap limits stop a `for` as they stop anything else. The
//! analyser (`analyse`) finds every call in the compiled AST and where its
//! arguments came from; those calls decide the tier, and are what `show` lists and
//! all the run is given.

use crate::analyse::{self, Plan};
use crate::engine::{self, Line, Stop};
use crate::undo::{self, Done, Inverse, Journal};
use crate::{Config, Consent, FsChange, FsRead, Host, Proof, data, show};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::Write;
use kabi::{MAX_METRIC_PROCESSES, Metrics, policy};
use llmproto::script::{Answer, Effect, check_summary, check_text};
use rhai::{AST, Array, Dynamic, Engine, EvalAltResult, INT, Map, Scope};

/// A script that passed every check, waiting for the user's answer.
#[derive(Debug)]
pub struct Checked {
    pub summary: String,
    pub script: String,
    pub reads: Vec<&'static str>,
    pub changes: Vec<&'static str>,
    /// Every call the script can make, and where its values come from.
    pub plan: Rc<Plan>,
    /// Set when the trial run stopped early on a stand-in value.
    pub note: Option<String>,
    /// Set when the person's own line made the call (`Shell::open_line`), not a
    /// model, a script or a goal: a window it opens takes the keys as a click
    /// would (`UiCall::Open`).
    pub person: bool,
    ast: AST,
}

impl Checked {
    /// A call shown to the user that no script engine runs: the package store
    /// (`crate::packages`) runs it; its text, and nothing to run.
    pub(crate) fn shown_only(call: &str) -> Self {
        Self {
            summary: String::new(),
            script: call.to_string(),
            reads: Vec::new(),
            changes: Vec::new(),
            plan: Rc::default(),
            note: None,
            person: false,
            ast: AST::empty(),
        }
    }
}

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

mod apps;
mod bind;
mod grants;
mod known;
mod look;
mod net;
mod stub;
mod windows;

use bind::*;
pub(crate) use bind::{file_done, home_path};
use stub::*;

fn rhai_error(message: impl Into<String>) -> Box<EvalAltResult> {
    let message: String = message.into();
    message.into()
}

fn metrics(host: &dyn Host) -> RhaiResult<Metrics> {
    host.metrics()
        .map_err(|error| rhai_error(format!("metrics: {error:?}")))
}

fn policy_name(id: u64) -> &'static str {
    if id == policy::ROUND_ROBIN {
        "round-robin"
    } else {
        "interactive"
    }
}

/// An outbound effect, sent only once the whole run has succeeded.
pub struct Queued {
    pub action: &'static str,
    pub send: Box<dyn FnOnce() -> Result<(), String>>,
}

/// The effects a run queued to leave the machine (the transactional outbox).
pub type Outbox = Rc<RefCell<Vec<Queued>>>;

/// What one run's bindings share: the OS, the changes made so far, the plan each
/// live call is checked against, and the effects waiting to go out.
#[derive(Clone)]
struct Tx {
    host: Rc<dyn Host>,
    /// What the run's entry may show the owners of its asked calls.
    consent: Consent,
    journal: Journal,
    plan: Rc<Plan>,
    outbox: Outbox,
    /// The shell's line for the last app reply that failed its row's call
    /// (`<row>: the app refused`), with none of the reply's words (#831).
    app_failed: Rc<RefCell<Option<String>>>,
    /// The person's own line made this run's calls (`Checked::person`).
    person: bool,
    /// The row whose app call the app took and did not answer: it may have
    /// run, so the run's outcome is not known (audit K #19).
    unanswered: Rc<RefCell<Option<String>>>,
}

impl Tx {
    fn new(host: &Rc<dyn Host>, plan: &Rc<Plan>) -> Self {
        Self {
            host: Rc::clone(host),
            consent: Consent::default(),
            journal: Journal::default(),
            plan: Rc::clone(plan),
            outbox: Outbox::default(),
            app_failed: Rc::default(),
            person: false,
            unanswered: Rc::default(),
        }
    }

    /// Refuses a live call the analysed plan does not have: one more check that
    /// what runs is what was approved.
    fn approved(&self, name: &str, live: &[&str]) -> RhaiResult<()> {
        match self.plan.allows(name, live) {
            true => Ok(()),
            false => Err(rhai_error(format!(
                "{name}({}) is not a call the checked script makes",
                live.join(", ")
            ))),
        }
    }

    /// Records a change, or, for a resource this run changed already, keeps the
    /// first value it replaced: the snapshot is taken once, at the first change.
    fn changed(&self, done: Done) {
        let mut journal = self.journal.borrow_mut();
        // Each file change is a transaction of its own, so each keeps its own
        // inverse; `undo` restores them newest first.
        if matches!(done.inverse, Some(Inverse::Fs { .. } | Inverse::App { .. })) {
            journal.push(done);
            return;
        }
        match journal
            .iter_mut()
            .find(|d| d.owner == done.owner && d.res == done.res)
        {
            Some(first) => {
                first.after = done.after;
                first.inverse = match (first.inverse, done.inverse) {
                    (
                        Some(Inverse::Spec { policy, preset, .. }),
                        Some(Inverse::Spec { after, .. }),
                    ) => Some(Inverse::Spec {
                        policy,
                        preset,
                        after,
                    }),
                    _ => None,
                };
            }
            None => journal.push(done),
        }
    }
}

/// Registers the registry's script API rows named in `granted` on `engine`, each
/// checked against `tx`'s plan when it runs.
fn register(engine: &mut Engine, granted: &[&'static str], tx: &Tx) {
    for row in agentabi::BOUND
        .iter()
        .chain(&agentabi::FS_ROWS)
        .chain(&agentabi::UI_ROWS)
        .chain(&agentabi::GRANT_ROWS)
        .chain(&agentabi::NET_ROWS)
        .filter(|row| granted.contains(&row.name))
    {
        bind(engine, row.name, tx);
    }
    // An installed app's rows (P4), granted as any row is.
    let Some(apps) = tx.host.apps() else {
        return;
    };
    let rows: Vec<crate::apps::AppRow> = granted
        .iter()
        .filter_map(|name| apps.borrow().rows.find(name).cloned())
        .collect();
    for row in rows {
        apps::bind(engine, row, tx);
    }
}

/// The script API rows the shell has no binding for, tried on a bare engine; the
/// tests keep it empty.
pub fn unbound(host: &Rc<dyn Host>) -> Vec<&'static str> {
    let mut engine = Engine::new_raw();
    let tx = Tx::new(host, &Rc::default());
    agentabi::BOUND
        .iter()
        .map(|row| row.name)
        .filter(|&name| !bind(&mut engine, name, &tx))
        .collect()
}

/// The pure functions every strict engine has besides the OS functions it is
/// granted, from the registry's helper rows. They touch nothing, so they are
/// run-tier rows in `tiers::TIERS`.
pub const HELPERS: &[&str] = &helper_names();

const fn helper_names() -> [&'static str; agentabi::HELPERS.len()] {
    let mut out = [""; agentabi::HELPERS.len()];
    let mut i = 0;
    while i < out.len() {
        out[i] = agentabi::HELPERS[i].name;
        i += 1;
    }
    out
}

/// The strict engine for LLM-written scripts, with only `granted` OS functions.
fn strict(line: &Rc<Line>, config: &Config, granted: &[&'static str], tx: &Tx) -> Engine {
    let mut engine = engine::bounded(line, &tx.host, config);
    engine
        .set_allow_anonymous_fn(false)
        .set_strict_variables(true)
        .set_max_call_levels(8);
    for symbol in ["while", "loop", "do", "fn", "Fn", "call", "curry", "eval"] {
        engine.disable_symbol(symbol);
    }
    // Numbers are integers only, so rounding one down leaves it as it is; models
    // write `floor` for "rounded down" all the same. Both are in `HELPERS`.
    engine.register_fn(agentabi::FLOOR.name, |x: INT| x);
    engine.register_fn(agentabi::FIXED.name, fixed);
    data::register(&mut engine, line, config.max_output);
    register(&mut engine, granted, tx);
    engine
}

/// The plan's calls, reads and changes apart, each in the registry's order.
fn split(plan: &Plan, rows: &'static [agentabi::Action]) -> (Vec<&'static str>, Vec<&'static str>) {
    let names = plan.names();
    let apps = rows
        .iter()
        .filter(|row| matches!(row.owner, agentabi::Owner::App(_)));
    let pick = |effect| {
        agentabi::BOUND
            .iter()
            .chain(&agentabi::FS_ROWS)
            .chain(apps.clone())
            .chain(&agentabi::UI_ROWS)
            .chain(&agentabi::GRANT_ROWS)
            .filter(|f| f.effect() == effect && names.contains(&f.name))
            .map(|f| f.name)
            .collect()
    };
    (pick(Effect::Read), pick(Effect::Change))
}

/// Longest part of an error quoted in a confirmation.
const NOTE_ERROR_CHARS: usize = 40;

/// Errors from integer operators, which fail only for some numbers and may not on
/// the real ones. Their messages quote only the numbers.
const STAND_IN_ERRORS: &[&str] = &[
    "Division by zero:",
    "Modulo division by zero",
    "Integer raised to a negative power:",
    "Addition overflow:",
    "Subtraction overflow:",
    "Multiplication overflow:",
    "Negation overflow:",
    "Division overflow:",
    "Exponential overflow:",
];

/// Why a script was not proposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// What is wrong with it, fit to send back to the model for a repair.
    Script(String),
    /// Ctrl-C stopped the trial run, so nothing goes back to the model.
    Interrupted,
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Script(why) => f.write_str(why),
            Self::Interrupted => f.write_str("stopped: interrupted"),
        }
    }
}

/// The limits LLM-written scripts run under: the line limits with the smaller
/// script heap budget.
fn for_scripts(config: &Config) -> Config {
    Config {
        heap_budget: config.script_heap_budget,
        ..*config
    }
}

/// Checks an answer: text limits, the strict compile, then a trial run against the
/// stub OS so unknown functions, wrong arguments and type errors are caught before
/// the user is asked. The error text is fit to send back to the model.
pub fn check(answer: &Answer, host: &Rc<dyn Host>, config: &Config) -> Result<Checked, Refusal> {
    check_against(answer, host, config, &agentabi::SCRIPT_CALLS)
}

/// [`check`] for the one-line script a tagged call's row runs as: its calls are
/// found among every bound row, the call-only ones too (`agentabi::BOUND`).
pub fn check_call(
    answer: &Answer,
    host: &Rc<dyn Host>,
    config: &Config,
) -> Result<Checked, Refusal> {
    check_against(answer, host, config, &agentabi::BOUND)
}

/// [`check`], finding the script's calls among `rows`: a model's script sees only
/// the script API, so a call-only row is no call of it and fails its trial.
pub(crate) fn check_against(
    answer: &Answer,
    host: &Rc<dyn Host>,
    config: &Config,
    rows: &'static [agentabi::Action],
) -> Result<Checked, Refusal> {
    let config = &for_scripts(config);
    let refuse = |why: String| Refusal::Script(why);
    // The llm service checked both already; the shell does not rely on that, since
    // the summary goes straight into the confirmation.
    let summary = check_summary(&answer.summary).map_err(|error| refuse(error.describe()))?;
    let script = check_text(&answer.script).map_err(|error| refuse(error.describe()))?;
    let line = Line::new();
    let stub: Rc<dyn Host> = Rc::new(Stub(Rc::clone(host)));
    let ast = strict(&line, config, &[], &Tx::new(&stub, &Rc::default()))
        .compile(&script)
        .map_err(|error| refuse(format!("syntax error: {error}")))?;
    let plan = Rc::new(analyse::analyse(&ast, rows));
    let (reads, changes) = split(&plan, rows);
    let granted = plan.names();
    let mut trial = strict(&line, config, &granted, &Tx::new(&stub, &plan));
    let functions = known::known(&trial);
    let missed = known::watch(&mut trial, Rc::clone(&functions));
    if let Some(error) = known::unknown_in_try(&ast, &functions) {
        return Err(refuse(format!("the script failed a trial run: {error}")));
    }
    line.begin(stub.as_ref(), config);
    let note = match engine::eval(&trial, &mut Scope::new(), &ast, config.exec) {
        Err(_) if line.stopped.get() == Some(Stop::Interrupted) => {
            return Err(Refusal::Interrupted);
        }
        // A call no function takes, though the script's own `catch` caught it.
        // This arm stays Ok-only: an uncaught error keeps the engine's own
        // words, line and position, and `known::takes` may record a miss for a
        // call that failed by itself (it cannot always tell), which only an
        // uncaught error shows rightly. The two depend on each other.
        Ok(_) if missed.borrow().is_some() => {
            let error = missed.take().unwrap_or_default();
            return Err(refuse(format!("the script failed a trial run: {error}")));
        }
        Ok(_) => None,
        Err(error) => match stand_in_note(&error) {
            // A stand-in's arithmetic stopped the trial after a `catch` hid a
            // call no function takes: the miss is what refuses it.
            Some(_) if missed.borrow().is_some() => {
                let error = missed.take().unwrap_or_default();
                return Err(refuse(format!("the script failed a trial run: {error}")));
            }
            Some(note) => Some(note),
            None => {
                return Err(refuse(format!(
                    "the script failed a trial run: {}",
                    line.describe(&error, config)
                )));
            }
        },
    };
    Ok(Checked {
        summary,
        script,
        reads,
        changes,
        plan,
        note,
        person: false,
        ast,
    })
}

/// The confirmation's note for a trial stopped by operator arithmetic, which may be
/// fine on the real numbers; `None` for every other error, which refuses the script.
fn stand_in_note(error: &EvalAltResult) -> Option<String> {
    let EvalAltResult::ErrorArithmetic(why, at) = error.unwrap_inner() else {
        return None;
    };
    if !STAND_IN_ERRORS.iter().any(|start| why.starts_with(start)) {
        return None;
    }
    // Rhai leaves the position out for some operators.
    let place = at
        .line()
        .or(error.position().line())
        .map_or(String::from("early"), |line| format!("at line {line}"));
    // The note is one line of the confirmation whatever the message holds.
    let why: String = why
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    Some(format!(
        "trial stopped {place} on stand-in values ({}); later lines untried",
        show::truncate(&why, NOTE_ERROR_CHARS)
    ))
}

/// The one question a script that asks gets: what the model says it does, then
/// the shell's own account of the calls that ask, and `why`: they have no undo,
/// the request did not ask for them, or content picks their target. Two short
/// lines; `show` has the rest.
pub fn question(id: u64, checked: &Checked, asking: &[&str], why: &str) -> String {
    format!(
        "script {id}: {}
It changes {}{why}. Go ahead? y/n, or show",
        checked.summary,
        asking.join(", ")
    )
}

/// The question for a script whose asking calls only read from a site an
/// app was allowed: what it reads with, and `why` it asks.
pub fn reads(id: u64, checked: &Checked, asking: &[&str], why: &str) -> String {
    format!(
        "script {id}: {}
It reads from a site with {} ({why}). Go ahead? y/n, or show",
        checked.summary,
        asking.join(", ")
    )
}

/// The question for a script that would send read content out of the machine
/// to a host the request did not name (P-F): `from` says where it was read.
pub fn sends(id: u64, checked: &Checked, asking: &[&str], from: &str) -> String {
    format!(
        "script {id}: {}
It would send what Kai read from {from} out of this machine with {}. Go ahead? y/n, or show",
        checked.summary,
        asking.join(", ")
    )
}

/// A script as the user sees it: the shell's own account of what it uses first,
/// then what the model says, then every line.
pub fn shown(id: u64, checked: &Checked) -> String {
    let mut text = format!("script {id}: ");
    match (checked.reads.is_empty(), checked.changes.is_empty()) {
        (true, true) => text.push_str("uses no OS functions"),
        (false, true) => {
            let _ = write!(text, "reads {}", checked.reads.join(", "));
        }
        (true, false) => {
            let _ = write!(text, "CHANGES {}", checked.changes.join(", "));
        }
        (false, false) => {
            let _ = write!(
                text,
                "reads {}; CHANGES {}",
                checked.reads.join(", "),
                checked.changes.join(", ")
            );
        }
    }
    // Calls and act scripts have no model summary: the code says what they do.
    if !checked.summary.is_empty() {
        let _ = write!(text, "\nmodel says: {}", checked.summary);
    }
    if let Some(note) = &checked.note {
        let _ = write!(text, "\n{note}");
    }
    for (number, line) in checked.script.lines().enumerate() {
        let _ = write!(text, "\n{:>2} | {line}", number + 1);
    }
    text
}

/// What a run printed, how it ended, and what it changed.
pub struct Ran {
    pub ok: bool,
    /// The script's output, then its error if it failed.
    pub text: String,
    /// The first change to each resource, with the value it replaced. The caller
    /// puts them back when `ok` is false.
    pub done: Vec<Done>,
    /// The script's last value, run in the shell; `None` from a child.
    pub value: Option<Dynamic>,
    /// The sources of the Content it showed with `show`, each once: what it
    /// printed from them is the person's to read, not the model's (#831). A
    /// child shows none: the rows that read Content never run apart.
    pub shown: Vec<&'static str>,
    /// Why it failed, as the shell wrote it after the output: the engine's
    /// error or what was not sent. `None` when it did not fail, or from a child.
    pub failed: Option<String>,
    /// It failed after an app's reply failed a row's call: `text` shows the
    /// person the app's words, and `failed` is the shell's own line in their
    /// place, the one a model turn is remembered by (#831).
    pub withheld: bool,
    /// A row whose app call the app took and did not answer: the run may
    /// have changed that app's state, so it did not fail cleanly.
    pub unanswered: Option<String>,
}

/// Runs a script with the real OS, granted only the calls the plan found. Its
/// outbound effects go out only if the whole run succeeded.
pub fn run(checked: &Checked, host: &Rc<dyn Host>, config: &Config) -> Ran {
    run_as(checked, host, config, Consent::default())
}

/// [`run`] for log entry `consent.entry`, showing its asked calls' owners
/// `consent.proof`.
pub fn run_as(checked: &Checked, host: &Rc<dyn Host>, config: &Config, consent: Consent) -> Ran {
    let config = &for_scripts(config);
    let line = Line::new();
    let mut tx = Tx::new(host, &checked.plan);
    tx.consent = consent;
    tx.person = checked.person;
    let engine = strict(&line, config, &checked.plan.names(), &tx);
    line.begin(host.as_ref(), config);
    let result = engine::eval(&engine, &mut Scope::new(), &checked.ast, config.exec);
    let mut text = line.take_output();
    let mut last = None;
    let mut failed = None;
    let mut withheld = false;
    let mut ok = match result {
        Ok(value) => {
            if !value.is_unit() {
                text.push_str(&show::value(&value));
            }
            last = Some(value);
            true
        }
        Err(error) => {
            let why = line.describe(&error, config);
            text.push_str(&why);
            // Whatever error ended a run after an app's reply failed it may
            // carry that reply's words, even caught and thrown again: the
            // shell's own line stands in for it (#831).
            let app = tx.app_failed.take();
            withheld = app.is_some();
            failed = Some(app.unwrap_or(why));
            false
        }
    };
    let queued = core::mem::take(&mut *tx.outbox.borrow_mut());
    if ok {
        let sent = text.len();
        ok = flush(queued, &mut text);
        if !ok {
            failed = Some(text[sent..].to_string());
        }
    }
    let done = tx.journal.take();
    Ran {
        ok,
        text,
        done,
        value: last,
        shown: line.take_shown(),
        failed,
        withheld,
        unanswered: tx.unanswered.take(),
    }
}

/// Sends what a successful run queued, in order, stopping at the first that fails.
pub(crate) fn flush(queued: Vec<Queued>, text: &mut String) -> bool {
    for effect in queued {
        if let Err(why) = (effect.send)() {
            let _ = write!(text, "{}: not sent: {why}", effect.action);
            return false;
        }
    }
    true
}
