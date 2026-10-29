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
            ast: AST::empty(),
        }
    }
}

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

mod look;
mod windows;

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
}

impl Tx {
    fn new(host: &Rc<dyn Host>, plan: &Rc<Plan>) -> Self {
        Self {
            host: Rc::clone(host),
            consent: Consent::default(),
            journal: Journal::default(),
            plan: Rc::clone(plan),
            outbox: Outbox::default(),
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
        if matches!(done.inverse, Some(Inverse::Fs { .. })) {
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
        .filter(|row| granted.contains(&row.name))
    {
        bind(engine, row.name, tx);
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

/// Binds the script API row `name`, with the arguments and result its row declares;
/// false if the shell has no implementation for it.
fn bind(engine: &mut Engine, name: &'static str, tx: &Tx) -> bool {
    {
        let (h, tx) = (Rc::clone(&tx.host), tx.clone());
        match name {
            "cpus" => {
                engine.register_fn(name, move || -> RhaiResult<INT> {
                    tx.approved(name, &[])?;
                    Ok(metrics(h.as_ref())?.cpus as INT)
                });
            }
            "memory" => {
                engine.register_fn(name, move || -> RhaiResult<Map> {
                    tx.approved(name, &[])?;
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
                    tx.approved(name, &[])?;
                    Ok((metrics(h.as_ref())?.uptime_ns / 1_000_000) as INT)
                });
            }
            "processes" => {
                engine.register_fn(name, move || -> RhaiResult<Array> {
                    tx.approved(name, &[])?;
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
                    tx.approved(name, &[])?;
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
                    tx.approved(name, &[])?;
                    Ok(policy_name(metrics(h.as_ref())?.policy).into())
                });
            }
            "set_policy" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
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
                    tx.changed(Done {
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
            "process_stop" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    let count = h
                        .stop_process(which, tx.consent.proof)
                        .map_err(|why| rhai_error(format!("process_stop: {why}")))?;
                    tx.changed(stopped(
                        name,
                        "kernel",
                        "process",
                        which,
                        &format!("stopped ({count})"),
                        None,
                    ));
                    Ok(())
                });
            }
            "service_stop" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    h.stop_service(which, tx.consent.proof)
                        .map_err(|why| rhai_error(format!("service_stop: {why}")))?;
                    tx.changed(stopped(
                        name,
                        "supervisor",
                        "service",
                        which,
                        "stopped",
                        None,
                    ));
                    Ok(())
                });
            }
            "service_restart" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    h.restart_service(which, tx.consent.proof)
                        .map_err(|why| rhai_error(format!("service_restart: {why}")))?;
                    tx.changed(stopped(
                        name,
                        "supervisor",
                        "service",
                        which,
                        "restarted",
                        None,
                    ));
                    Ok(())
                });
            }
            "service_start" => {
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    tx.approved(name, &[which])?;
                    // No undo, no change: the tier promises one.
                    let undo = undo::Name::new(which).ok_or_else(|| {
                        rhai_error("service_start: no service has a name that long")
                    })?;
                    h.start_service(which, tx.consent.entry)
                        .map_err(|why| rhai_error(format!("service_start: {why}")))?;
                    tx.changed(stopped(
                        name,
                        "supervisor",
                        "service",
                        which,
                        "running",
                        Some(Inverse::Started(undo)),
                    ));
                    Ok(())
                });
            }
            name if name.starts_with("fs_") => return bind_file(engine, name, h, tx),
            "appearance_set" => {
                look::bind(engine, h, tx);
            }
            name if agentabi::UI_ROWS.iter().any(|r| r.name == name) => {
                return windows::bind(engine, name, h, tx);
            }
            _ => return false,
        }
    }
    true
}

/// Binds P2's file row `name` (task 11): reads hand back `Data`, and each change
/// is one `fs` transaction that `undo` restores.
fn bind_file(engine: &mut Engine, name: &'static str, h: Rc<dyn Host>, tx: Tx) -> bool {
    let fail = move |why: String| rhai_error(format!("{name}: {why}"));
    match name {
        "fs_list" | "fs_read" | "fs_history" => {
            engine.register_fn(name, move |path: &str| -> RhaiResult<data::Data> {
                tx.approved(name, &[path])?;
                let path = home(name, path)?;
                let read = match name {
                    "fs_list" => FsRead::List(path),
                    "fs_read" => FsRead::Read(path),
                    _ => FsRead::History(path),
                };
                h.fs_read(read)
                    .map(|text| data::Data::new("fs", text))
                    .map_err(fail)
            });
        }
        "fs_changes" => {
            engine.register_fn(name, move || -> RhaiResult<String> {
                tx.approved(name, &[])?;
                h.fs_read(FsRead::Changes).map_err(fail)
            });
        }
        "fs_create" | "fs_write" => {
            engine.register_fn(name, move |path: &str, text: &str| -> RhaiResult<()> {
                tx.approved(name, &[path, text])?;
                let path = home(name, path)?;
                let (change, before, after) = match name {
                    "fs_create" => (FsChange::Create { path, text }, "absent", "created"),
                    _ => (
                        FsChange::Write { path, text },
                        "its earlier text",
                        "written",
                    ),
                };
                let txn = h.fs_change(change).map_err(fail)?;
                tx.changed(file_done(name, path, before, after, Some(txn)));
                Ok(())
            });
        }
        "fs_move" => {
            engine.register_fn(name, move |from: &str, to: &str| -> RhaiResult<()> {
                tx.approved(name, &[from, to])?;
                let (from, to) = (home(name, from)?, home(name, to)?);
                let txn = h.fs_change(FsChange::Move { from, to }).map_err(fail)?;
                let after = format!("moved to {to}");
                tx.changed(file_done(name, from, "there", &after, Some(txn)));
                Ok(())
            });
        }
        "fs_trash" | "fs_restore" => {
            engine.register_fn(name, move |path: &str| -> RhaiResult<()> {
                tx.approved(name, &[path])?;
                let path = home(name, path)?;
                let (change, before, after) = match name {
                    "fs_trash" => (FsChange::Trash(path), "there", "in the trash"),
                    _ => (FsChange::Restore(path), "in the trash", "restored"),
                };
                let txn = h.fs_change(change).map_err(fail)?;
                tx.changed(file_done(name, path, before, after, Some(txn)));
                Ok(())
            });
        }
        "fs_empty_trash" => {
            engine.register_fn(name, move || -> RhaiResult<()> {
                tx.approved(name, &[])?;
                h.fs_change(FsChange::EmptyTrash).map_err(fail)?;
                tx.changed(file_done(name, "trash", "full", "emptied", None));
                Ok(())
            });
        }
        _ => return false,
    }
    true
}

/// `path` as the `fs` client takes it, relative to `/home/user`: a relative path
/// starts there, `/home/user/…` is the same, and any other absolute path
/// (`/system/log`) or a `..` is `None`. The AI's file rows reach only the user's
/// files, never `/system` (P2 plan §4.3); `fs`'s lookup refuses `..` too.
pub(crate) fn home_path(path: &str) -> Option<&str> {
    let rest = match path.strip_prefix('/') {
        None => path,
        Some(_) if path == "/home/user" => "",
        Some(_) => path.strip_prefix("/home/user/")?,
    };
    let escapes = rest.split('/').any(|part| part == "..");
    (!escapes).then_some(rest)
}

/// [`home_path`] for row `name`'s binding, refused in its words.
fn home<'a>(name: &str, path: &'a str) -> RhaiResult<&'a str> {
    home_path(path).ok_or_else(|| rhai_error(format!("{name}: {path} is not under /home/user")))
}

/// One file change: `file notes.txt created (was absent)`, undone by restoring
/// transaction `txn`.
fn file_done(
    action: &'static str,
    path: &str,
    before: &str,
    after: &str,
    txn: Option<u64>,
) -> Done {
    Done {
        action: action.into(),
        owner: "fs".into(),
        res: format!("file {path}"),
        before: before.into(),
        after: after.into(),
        inverse: txn.map(|txn| Inverse::Fs { txn }),
    }
}

/// One process or service change: `service notes running (was stopped)`.
fn stopped(
    action: &'static str,
    owner: &str,
    kind: &str,
    which: &str,
    after: &str,
    inverse: Option<Inverse>,
) -> Done {
    Done {
        action: action.into(),
        owner: owner.into(),
        res: format!("{kind} {which}"),
        before: String::from(if after == "running" {
            "stopped"
        } else {
            "running"
        }),
        after: after.into(),
        inverse,
    }
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
fn split(plan: &Plan) -> (Vec<&'static str>, Vec<&'static str>) {
    let names = plan.names();
    let pick = |effect| {
        agentabi::BOUND
            .iter()
            .chain(&agentabi::FS_ROWS)
            .chain(&agentabi::UI_ROWS)
            .filter(|f| f.effect() == effect && names.contains(&f.name))
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
    fn stop_process(&self, _: &str, _: Proof) -> Result<u64, String> {
        Ok(1)
    }
    fn stop_service(&self, _: &str, _: Proof) -> Result<(), String> {
        Ok(())
    }
    fn start_service(&self, _: &str, _: u64) -> Result<(), String> {
        Ok(())
    }

    fn restart_service(&self, _: &str, _: Proof) -> Result<(), String> {
        Ok(())
    }
    fn fs_read(&self, _: FsRead) -> Result<String, String> {
        Ok(String::new())
    }
    fn fs_change(&self, _: FsChange) -> Result<u64, String> {
        Ok(0)
    }
    fn appearance_read(&self) -> Option<String> {
        self.0.appearance_read()
    }
    fn appearance_write(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn ui(&self, call: &crate::ui::UiCall) -> Result<String, String> {
        use crate::ui::UiCall;
        Ok(String::from(match call {
            UiCall::Windows => "w1\tx\tx\t1",
            UiCall::Focus(_) => "none",
            UiCall::Close(window) => return Ok(window.clone()),
            _ => "",
        }))
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
    let (reads, changes) = split(&plan);
    let granted = plan.names();
    let trial = strict(&line, config, &granted, &Tx::new(&stub, &plan));
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
        plan,
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
    let engine = strict(&line, config, &checked.plan.names(), &tx);
    line.begin(host.as_ref(), config);
    let result = engine::eval(&engine, &mut Scope::new(), &checked.ast, config.exec);
    let mut text = line.take_output();
    let mut ok = match result {
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
    let queued = core::mem::take(&mut *tx.outbox.borrow_mut());
    if ok {
        ok = flush(queued, &mut text);
    }
    let done = tx.journal.take();
    Ran { ok, text, done }
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
