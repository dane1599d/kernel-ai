//! LLM-written scripts: checked in a strict profile, tried against stub functions,
//! and run in a fresh engine that has only the functions the script names: at
//! once if every function it names is run-tier, after a yes if one asks (see
//! `tiers`). Each change records the value it replaced, for `undo`.
//!
//! The strict profile has no closures, no `fn`, no indirect calls (`Fn`, `call`,
//! `curry`, `eval`) and no undefined variables, so a script can only reach an OS
//! function by writing its name. Its only loop is `for x in <array, string or
//! range>`, which ends by itself; `while`, `loop` and `do` stay off, and the
//! operation, time and heap limits stop a `for` as they stop anything else. The names found in the text
//! (strings and comments included, so the list can only be too long, never too
//! short) decide the tier, and are what `show` lists and all the run is given.

use crate::engine::{self, Line, Stop};
use crate::undo::{self, Done, Inverse, Journal};
use crate::{Config, Host, show};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use kabi::{MAX_METRIC_PROCESSES, Metrics, policy};
use llmproto::script::{API, Answer, ApiFn, Effect, check_summary, check_text};
use rhai::{AST, Array, Dynamic, Engine, EvalAltResult, INT, Map, Scope};

/// A script that passed every check, waiting for the user's answer.
#[derive(Debug)]
pub struct Checked {
    pub summary: String,
    pub script: String,
    pub reads: Vec<&'static str>,
    pub changes: Vec<&'static str>,
    /// Set when the trial run stopped early on a stand-in value.
    pub note: Option<String>,
    ast: AST,
}

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

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

/// Registers the registry's script API rows named in `granted` on `engine`, backed
/// by `host`. Each change goes into `journal` with how to undo it.
fn register(engine: &mut Engine, host: &Rc<dyn Host>, granted: &[&'static str], journal: &Journal) {
    for row in API.iter().filter(|row| granted.contains(&row.name)) {
        bind(engine, row.name, host, journal);
    }
}

/// The script API rows the shell has no binding for, tried on a bare engine; the
/// tests keep it empty.
pub fn unbound(host: &Rc<dyn Host>) -> Vec<&'static str> {
    let mut engine = Engine::new_raw();
    let journal = Journal::default();
    API.iter()
        .map(|row| row.name)
        .filter(|&name| !bind(&mut engine, name, host, &journal))
        .collect()
}

/// Binds the script API row `name`, with the arguments and result its row declares;
/// false if the shell has no implementation for it.
fn bind(engine: &mut Engine, name: &'static str, host: &Rc<dyn Host>, journal: &Journal) -> bool {
    {
        let h = Rc::clone(host);
        match name {
            "cpus" => {
                engine.register_fn(name, move || -> RhaiResult<INT> {
                    Ok(metrics(h.as_ref())?.cpus as INT)
                });
            }
            "memory" => {
                engine.register_fn(name, move || -> RhaiResult<Map> {
                    let m = metrics(h.as_ref())?;
                    let mut map = Map::new();
                    map.insert(
                        "free_mib".into(),
                        ((m.free_frames * 4 / 1024) as INT).into(),
                    );
                    map.insert(
                        "total_mib".into(),
                        ((m.total_frames * 4 / 1024) as INT).into(),
                    );
                    map.insert("heap_kib".into(), ((m.heap_bytes / 1024) as INT).into());
                    Ok(map)
                });
            }
            "uptime_ms" => {
                engine.register_fn(name, move || -> RhaiResult<INT> {
                    Ok((metrics(h.as_ref())?.uptime_ns / 1_000_000) as INT)
                });
            }
            "processes" => {
                engine.register_fn(name, move || -> RhaiResult<Array> {
                    let m = metrics(h.as_ref())?;
                    let list = m.processes.iter().take(m.process_count as usize).map(|p| {
                        let mut map = Map::new();
                        map.insert("pid".into(), (p.pid as INT).into());
                        map.insert("name".into(), p.name().to_string().into());
                        Dynamic::from_map(map)
                    });
                    Ok(list.collect())
                });
            }
            "threads" => {
                engine.register_fn(name, move || -> RhaiResult<Map> {
                    let m = metrics(h.as_ref())?;
                    let mut map = Map::new();
                    map.insert("switches".into(), (m.context_switches as INT).into());
                    map.insert("ejections".into(), (m.ejections as INT).into());
                    map.insert("migrations".into(), (m.migrations as INT).into());
                    Ok(map)
                });
            }
            "get_policy" => {
                engine.register_fn(name, move || -> RhaiResult<String> {
                    Ok(policy_name(metrics(h.as_ref())?.policy).into())
                });
            }
            "set_policy" => {
                let journal = Rc::clone(journal);
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    let id = match which {
                        "round-robin" | "rr" => policy::ROUND_ROBIN,
                        "interactive" => policy::INTERACTIVE,
                        other => return Err(rhai_error(format!("unknown policy {other:?}"))),
                    };
                    // No undo, no change: the tier promises one.
                    let (was, preset) = undo::current_spec(h.as_ref()).ok_or_else(|| {
                        rhai_error("set_policy: the installed policy could not be put back, so it was left as it is")
                    })?;
                    h.set_policy(id)
                        .map_err(|error| rhai_error(format!("set_policy: {error:?}")))?;
                    // What runs now, so `undo` can tell whether it moved since.
                    let after = undo::current_spec(h.as_ref()).unwrap_or((id, preset));
                    journal.borrow_mut().push(Done {
                        action: "set_policy".into(),
                        owner: "kernel".into(),
                        res: "policy".into(),
                        before: undo::spec_name(was, preset),
                        after: policy_name(id).into(),
                        inverse: Some(Inverse::Spec {
                            policy: was,
                            preset,
                            after,
                        }),
                    });
                    Ok(())
                });
            }
            _ => return false,
        }
    }
    true
}

/// Most decimals [`fixed`] writes.
const MAX_FIXED_PLACES: INT = 9;

/// `n / 10^places` as text with exactly `places` decimals: `fixed(4321, 3)` is
/// "4.321". Numbers are integers only, so this is how a script shows a fraction.
fn fixed(n: INT, places: INT) -> RhaiResult<String> {
    if !(0..=MAX_FIXED_PLACES).contains(&places) {
        return Err(rhai_error(format!(
            "fixed: places must be 0 to {MAX_FIXED_PLACES}, not {places}"
        )));
    }
    let scale = 10u64.pow(places as u32);
    let sign = if n < 0 { "-" } else { "" };
    let (whole, part) = (n.unsigned_abs() / scale, n.unsigned_abs() % scale);
    Ok(match places {
        0 => format!("{sign}{whole}"),
        _ => format!("{sign}{whole}.{part:0width$}", width = places as usize),
    })
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
fn strict(
    line: &Rc<Line>,
    host: &Rc<dyn Host>,
    config: &Config,
    granted: &[&'static str],
    journal: &Journal,
) -> Engine {
    let mut engine = engine::bounded(line, host, config);
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
    register(&mut engine, host, granted, journal);
    engine
}

/// The API functions named anywhere in `script`, reads and changes apart.
pub fn names_in(script: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    let words: Vec<&str> = script
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .collect();
    let used = |f: &&ApiFn| words.contains(&f.name);
    let pick = |effect| {
        API.iter()
            .filter(|f| f.effect() == effect)
            .filter(used)
            .map(|f| f.name)
            .collect()
    };
    (pick(Effect::Read), pick(Effect::Change))
}

/// Stand-in for the OS during the trial run. Nothing changes, and the time, heap and
/// Ctrl-C checks use the real host, so the trial is bounded like any other run.
/// Its numbers are all non-zero and its list is as long as the real one can be, with
/// names as long as real names can be, so an index or a division that works on the
/// real system works here too.
struct Stub(Rc<dyn Host>);

impl Host for Stub {
    fn now_ns(&self) -> u64 {
        self.0.now_ns()
    }
    fn interrupted(&self) -> bool {
        self.0.interrupted()
    }
    fn heap_bytes(&self) -> u64 {
        self.0.heap_bytes()
    }
    fn metrics(&self) -> Result<Metrics, kabi::Error> {
        let mut m = Metrics::empty();
        m.cpus = 2;
        m.free_frames = 256 * 64;
        m.total_frames = 256 * 128;
        m.heap_bytes = 64 * 1024;
        m.uptime_ns = 1_000_000_000;
        m.context_switches = 1;
        m.ejections = 1;
        m.migrations = 1;
        for (index, slot) in m.processes.iter_mut().enumerate() {
            slot.pid = index as u64 + 2;
            slot.name = [b'x'; kabi::MAX_NAME];
            slot.name_len = kabi::MAX_NAME as u64;
        }
        m.process_count = MAX_METRIC_PROCESSES as u64;
        Ok(m)
    }
    fn set_policy(&self, _: u64) -> Result<(), kabi::Error> {
        Ok(())
    }
    fn ask(&self, _: &str) -> Result<String, String> {
        Err("not available to scripts".into())
    }
    fn script(
        &self,
        _: &llmproto::script::ScriptRequest,
    ) -> Result<Answer, llmproto::script::AnswerError> {
        Err(llmproto::script::AnswerError::Server(
            "not available to scripts".into(),
        ))
    }
    fn audit(&self, _: &str) {}
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
    let config = &for_scripts(config);
    let refuse = |why: String| Refusal::Script(why);
    // The llm service checked both already; the shell does not rely on that, since
    // the summary goes straight into the confirmation.
    let summary = check_summary(&answer.summary).map_err(|error| refuse(error.describe()))?;
    let script = check_text(&answer.script).map_err(|error| refuse(error.describe()))?;
    let (reads, changes) = names_in(&script);
    let granted: Vec<&'static str> = reads.iter().chain(&changes).copied().collect();
    let line = Line::new();
    let stub: Rc<dyn Host> = Rc::new(Stub(Rc::clone(host)));
    let trial = strict(&line, &stub, config, &granted, &Journal::default());
    let ast = trial
        .compile(&script)
        .map_err(|error| refuse(format!("syntax error: {error}")))?;
    line.begin(stub.as_ref(), config);
    let note = match engine::eval(&trial, &mut Scope::new(), &ast, config.exec) {
        Ok(_) => None,
        Err(_) if line.stopped.get() == Some(Stop::Interrupted) => {
            return Err(Refusal::Interrupted);
        }
        Err(error) => match stand_in_note(&error) {
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
        note,
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
/// the shell's own account of the functions it calls that ask, and why: they have
/// no undo, or (`added`) the request did not ask for them. Two short lines; `show`
/// has the rest.
pub fn question(id: u64, checked: &Checked, asking: &[&str], added: bool) -> String {
    let why = match added {
        true => ", which the request did not ask for",
        false => " (no undo)",
    };
    format!(
        "script {id}: {}\nIt changes {}{why}. Go ahead? y/n, or show",
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
    pub done: Vec<Done>,
}

/// Runs a script with the real OS, granted only what it names.
pub fn run(checked: &Checked, host: &Rc<dyn Host>, config: &Config) -> Ran {
    let config = &for_scripts(config);
    let granted: Vec<&'static str> = checked
        .reads
        .iter()
        .chain(&checked.changes)
        .copied()
        .collect();
    let line = Line::new();
    let journal = Journal::default();
    let engine = strict(&line, host, config, &granted, &journal);
    line.begin(host.as_ref(), config);
    let result = engine::eval(&engine, &mut Scope::new(), &checked.ast, config.exec);
    let mut text = line.take_output();
    let ok = match result {
        Ok(value) => {
            if !value.is_unit() {
                text.push_str(&show::value(&value));
            }
            true
        }
        Err(error) => {
            text.push_str(&line.describe(&error, config));
            false
        }
    };
    let done = journal.take();
    Ran { ok, text, done }
}
