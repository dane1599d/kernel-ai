//! The OS functions the shell offers, both as Rhai functions (`mem()`) and as plain
//! commands (`mem`, `policy rr`, `ask how many CPUs are there`).

use crate::{Host, Line, runtime_error};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use core::fmt::Write;
use kabi::{Metrics, policy};
use rhai::{Engine, EvalAltResult};

/// (name, what it does), in the order `help` lists them.
pub const COMMANDS: &[(&str, &str)] = &[
    ("help", "this list"),
    ("mem", "free memory and heap use"),
    ("cpus", "CPUs online"),
    ("threads", "scheduler statistics"),
    ("uptime", "time since boot"),
    ("ps", "running services"),
    (
        "policy",
        "show, or switch with `policy rr` / `policy interactive`",
    ),
    ("ask", "`ask <question>` sends the question to the LLM"),
];

/// Characters that, right after a command's name, mean the line is Rhai code using
/// that name rather than the command with words after it.
const RHAI_AFTER_NAME: &str = "(=.[+-*/%<>!&|^?;,";

/// If `line` is a plain command, its name and the rest of the line. A line is a
/// command when its first word is one, that word is not a variable, and what
/// follows is words rather than Rhai syntax.
pub fn split_command(
    line: &str,
    is_variable: impl Fn(&str) -> bool,
) -> Option<(&'static str, &str)> {
    let (word, rest) = match line.find(char::is_whitespace) {
        Some(at) => (&line[..at], line[at..].trim_start()),
        None => (line, ""),
    };
    let name = COMMANDS.iter().find(|(name, _)| *name == word)?.0;
    if is_variable(name) {
        return None;
    }
    match rest.chars().next() {
        Some(first) if RHAI_AFTER_NAME.contains(first) => None,
        _ => Some((name, rest)),
    }
}

/// Runs a plain command.
pub fn run(host: &dyn Host, name: &str, rest: &str) -> Result<String, String> {
    let no_arguments = || {
        if rest.is_empty() {
            Ok(())
        } else {
            Err(format!("{name} takes no arguments"))
        }
    };
    match name {
        "help" => {
            no_arguments()?;
            Ok(help())
        }
        "mem" => no_arguments().and_then(|()| mem(host)),
        "cpus" => no_arguments().and_then(|()| metrics(host).map(|m| format!("{}", m.cpus))),
        "threads" => no_arguments().and_then(|()| threads(host)),
        "uptime" => no_arguments().and_then(|()| uptime(host)),
        "ps" => no_arguments().and_then(|()| ps(host)),
        "policy" if rest.is_empty() => current_policy(host),
        "policy" => set_policy(host, rest),
        "ask" if rest.is_empty() => Err("usage: ask <question>".into()),
        "ask" => host.ask(rest),
        other => Err(format!("unknown command {other}")),
    }
}

fn help() -> String {
    let mut text = String::new();
    for (name, what) in COMMANDS {
        let _ = writeln!(text, "{name:<8} {what}");
    }
    text.push_str("Anything else is Rhai: let x = 6; fn sq(n) { n * n }; sq(x).\n");
    text.push_str("Functions see only their parameters. `reset` clears everything.");
    text
}

fn metrics(host: &dyn Host) -> Result<Metrics, String> {
    host.metrics()
        .map_err(|error| format!("metrics: {error:?}"))
}

fn mem(host: &dyn Host) -> Result<String, String> {
    let m = metrics(host)?;
    Ok(format!(
        "{} of {} MiB free; kernel heap: {} allocations, {} KiB",
        m.free_frames * 4 / 1024,
        m.total_frames * 4 / 1024,
        m.heap_allocations,
        m.heap_bytes / 1024
    ))
}

fn policy_name(id: u64) -> &'static str {
    match id {
        policy::ROUND_ROBIN => "round-robin",
        _ => "interactive",
    }
}

fn threads(host: &dyn Host) -> Result<String, String> {
    let m = metrics(host)?;
    let mut text = format!(
        "policy {}; {} context switches; {} ejections; {} migrations",
        policy_name(m.policy),
        m.context_switches,
        m.ejections,
        m.migrations
    );
    if m.running_policy != m.policy {
        let _ = write!(
            text,
            " (watchdog fallback: {} running)",
            policy_name(m.running_policy)
        );
    }
    Ok(text)
}

fn uptime(host: &dyn Host) -> Result<String, String> {
    let ms = metrics(host)?.uptime_ns / 1_000_000;
    Ok(format!("{}.{:03} s", ms / 1000, ms % 1000))
}

fn ps(host: &dyn Host) -> Result<String, String> {
    let m = metrics(host)?;
    let mut text = String::new();
    for process in m.processes.iter().take(m.process_count as usize) {
        let _ = writeln!(text, "{:>5}  {}", process.pid, process.name());
    }
    if m.processes_omitted > 0 {
        let _ = writeln!(text, "({} more not shown)", m.processes_omitted);
    }
    Ok(String::from(text.trim_end()))
}

fn current_policy(host: &dyn Host) -> Result<String, String> {
    Ok(format!(
        "scheduling policy: {}",
        policy_name(metrics(host)?.policy)
    ))
}

fn set_policy(host: &dyn Host, name: &str) -> Result<String, String> {
    let id = match name.trim() {
        "rr" | "round-robin" => policy::ROUND_ROBIN,
        "interactive" => policy::INTERACTIVE,
        other => return Err(format!("unknown policy {other}: use interactive or rr")),
    };
    host.set_policy(id)
        .map_err(|error| format!("policy: {error:?}"))?;
    Ok(format!("scheduling policy: {}", policy_name(id)))
}

/// Registers the commands as Rhai functions.
pub(crate) fn register(engine: &mut Engine, host: &Rc<dyn Host>, line: &Rc<Line>, budget_ms: u64) {
    type Out = Result<String, alloc::boxed::Box<EvalAltResult>>;
    let wrap = |result: Result<String, String>| -> Out { result.map_err(runtime_error) };
    engine.register_fn("help", help);
    for (name, f) in [
        ("mem", mem as fn(&dyn Host) -> Result<String, String>),
        ("threads", threads),
        ("uptime", uptime),
        ("ps", ps),
        ("policy", current_policy),
    ] {
        let host = Rc::clone(host);
        engine.register_fn(name, move || -> Out { wrap(f(host.as_ref())) });
    }
    let h = Rc::clone(host);
    engine.register_fn(
        "cpus",
        move || -> Result<i64, alloc::boxed::Box<EvalAltResult>> {
            metrics(h.as_ref())
                .map(|m| m.cpus as i64)
                .map_err(runtime_error)
        },
    );
    let h = Rc::clone(host);
    engine.register_fn("policy", move |name: &str| -> Out {
        wrap(set_policy(h.as_ref(), name))
    });
    let h = Rc::clone(host);
    let watch = Rc::clone(line);
    engine.register_fn("ask", move |prompt: &str| -> Out {
        let answer = h.ask(prompt);
        // Time spent waiting for the LLM does not count against the line.
        watch
            .deadline_ns
            .set(h.now_ns().saturating_add(budget_ms * 1_000_000));
        wrap(answer)
    });
}
