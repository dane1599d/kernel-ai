//! LLM-written scripts: checked in a strict profile, tried against stub functions,
//! shown in full, and run only after a yes, in a fresh engine that has only the
//! functions the script names.
//!
//! The strict profile has no loops of any kind, no closures, no `fn`, no indirect
//! calls (`Fn`, `call`, `curry`, `eval`) and no undefined variables, so a script
//! can only reach an OS function by writing its name. The names found in the text
//! (strings and comments included, so the list can only be too long, never too
//! short) are what the confirmation shows and all the run is given.

use crate::engine::{self, Line};
use crate::{Config, Host, Reply, show};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use kabi::{Metrics, policy};
use llmproto::script::{API, Answer, ApiFn, Effect, check_text};
use rhai::{AST, Array, Dynamic, Engine, EvalAltResult, INT, Map, Scope};

/// A script that passed every check, waiting for the user's answer.
#[derive(Debug)]
pub struct Checked {
    pub summary: String,
    pub script: String,
    pub reads: Vec<&'static str>,
    pub changes: Vec<&'static str>,
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

/// Registers the API functions in `granted` on `engine`, backed by `host`.
fn register(engine: &mut Engine, host: &Rc<dyn Host>, granted: &[&'static str]) {
    for &name in granted {
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
                engine.register_fn(name, move |which: &str| -> RhaiResult<()> {
                    let id = match which {
                        "round-robin" | "rr" => policy::ROUND_ROBIN,
                        "interactive" => policy::INTERACTIVE,
                        other => return Err(rhai_error(format!("unknown policy {other:?}"))),
                    };
                    h.set_policy(id)
                        .map_err(|error| rhai_error(format!("set_policy: {error:?}")))
                });
            }
            _ => {}
        }
    }
}

/// The strict engine for LLM-written scripts, with only `granted` OS functions.
fn strict(
    line: &Rc<Line>,
    host: &Rc<dyn Host>,
    config: &Config,
    granted: &[&'static str],
) -> Engine {
    let mut engine = engine::bounded(line, host, config);
    engine
        .set_allow_looping(false)
        .set_allow_anonymous_fn(false)
        .set_strict_variables(true)
        .set_max_call_levels(8);
    for symbol in ["fn", "Fn", "call", "curry", "eval"] {
        engine.disable_symbol(symbol);
    }
    register(&mut engine, host, granted);
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
            .filter(|f| f.effect == effect)
            .filter(used)
            .map(|f| f.name)
            .collect()
    };
    (pick(Effect::Read), pick(Effect::Change))
}

/// Stand-in for the OS during the trial run: plausible values, and nothing changes.
struct Stub;

impl Host for Stub {
    fn now_ns(&self) -> u64 {
        0
    }
    fn interrupted(&self) -> bool {
        false
    }
    fn heap_bytes(&self) -> u64 {
        0
    }
    fn metrics(&self) -> Result<Metrics, kabi::Error> {
        let mut m = Metrics::empty();
        m.cpus = 2;
        m.free_frames = 256 * 64;
        m.total_frames = 256 * 128;
        m.heap_bytes = 64 * 1024;
        m.uptime_ns = 1_000_000_000;
        m.context_switches = 1;
        m.process_count = 1;
        m.processes[0].pid = 1;
        m.processes[0].name[..5].copy_from_slice(b"shell");
        m.processes[0].name_len = 5;
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

/// Checks an answer: text limits, the strict compile, then a trial run against the
/// stub OS so unknown functions, wrong arguments and type errors are caught before
/// the user is asked. The error text is fit to send back to the model.
pub fn check(answer: &Answer, config: &Config) -> Result<Checked, String> {
    let script = check_text(&answer.script).map_err(|error| error.describe())?;
    let (reads, changes) = names_in(&script);
    let granted: Vec<&'static str> = reads.iter().chain(&changes).copied().collect();
    let line = Line::new();
    let stub: Rc<dyn Host> = Rc::new(Stub);
    let trial = strict(&line, &stub, config, &granted);
    let ast = trial
        .compile(&script)
        .map_err(|error| format!("syntax error: {error}"))?;
    line.begin(stub.as_ref(), config);
    // The stub clock never moves, so only the operation backstop bounds the trial.
    if let Err(error) = trial.eval_ast_with_scope::<Dynamic>(&mut Scope::new(), &ast) {
        return Err(format!("the script failed a trial run: {error}"));
    }
    Ok(Checked {
        summary: answer.summary.clone(),
        script,
        reads,
        changes,
        ast,
    })
}

/// What the confirmation shows: the shell's own account of what the script uses
/// first, then what the model says, then every line, then the question last.
pub fn confirmation(id: u64, checked: &Checked) -> String {
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
    let _ = write!(text, "\nmodel says: {}", checked.summary);
    for (number, line) in checked.script.lines().enumerate() {
        let _ = write!(text, "\n{:>2} | {line}", number + 1);
    }
    text.push_str("\nrun it? yes/no");
    text
}

/// Runs a confirmed script with the real OS, granted only what it names.
pub fn run(checked: &Checked, host: &Rc<dyn Host>, config: &Config) -> Reply {
    let granted: Vec<&'static str> = checked
        .reads
        .iter()
        .chain(&checked.changes)
        .copied()
        .collect();
    let line = Line::new();
    let engine = strict(&line, host, config, &granted);
    line.begin(host.as_ref(), config);
    let result = engine.eval_ast_with_scope::<Dynamic>(&mut Scope::new(), &checked.ast);
    let mut text = line.take_output();
    match result {
        Ok(value) => {
            if !value.is_unit() {
                text.push_str(&show::value(&value));
            }
            Reply::ok(text)
        }
        Err(error) => {
            text.push_str(&line.describe(&error, config));
            Reply::err(text)
        }
    }
}
