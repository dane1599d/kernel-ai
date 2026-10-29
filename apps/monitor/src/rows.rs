//! The OS rows the monitor asks for, as effects, and what their results mean.
//! Every row is a read: stopping a process is a different row, `process_stop`,
//! which asks. A structured result is WAVE text typed by the row's `RetType`
//! (`agentabi/src/rows.rs`); the monitor reads it with `appabi::wave`, so a
//! hostile or newer result is an error, never a panic.
//!
//! `processes` and `model_state` are asked for fields the OS does not give yet
//! (`cpu_permille`, `mem_kib`; the model's state and tokens/s). A field left out
//! reads as unknown and the window shows "-", so the monitor works with the rows
//! as they are and fills in when the rows grow.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use appabi::{Ty, Val, wave};

/// Most processes read from one answer.
pub const MAX_PROCS: usize = 256;
/// Longest process name kept, in characters.
pub const NAME_CHARS: usize = 48;

/// One effect: an OS row by name and its arguments (a WAVE record).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub name: &'static str,
    pub args: String,
}

fn read(name: &'static str) -> Call {
    Call {
        name,
        args: String::from("{}"),
    }
}

pub fn processes() -> Call {
    read("processes")
}

pub fn memory() -> Call {
    read("memory")
}

pub fn cpus() -> Call {
    read("cpus")
}

pub fn uptime() -> Call {
    read("uptime_ms")
}

pub fn model_state() -> Call {
    read("model_state")
}

/// `process_stop`: stops or kills processes by name. It asks, and nothing undoes
/// it. `None` for a name a row would not take.
pub fn stop(name: &str) -> Option<Call> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\');
    ok.then(|| Call {
        name: "process_stop",
        args: alloc::format!("{{name: \"{name}\"}}"),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    /// The name as the window shows it: no control characters, cut short.
    pub name: String,
    /// Whether `name` is the name the OS gave. `process_stop` takes a name, so
    /// only a process whose name is exact can be stopped from here.
    pub exact: bool,
    /// CPU over the last window, in thousandths of one CPU.
    pub cpu_permille: Option<u32>,
    pub mem_kib: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub free_mib: u64,
    pub total_mib: u64,
    pub heap_kib: u64,
}

/// What the model is doing, as `model_state` says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub state: String,
    /// Decode speed, in tenths of a token a second.
    pub tokens_per_s_x10: Option<u32>,
}

fn int() -> Ty {
    Ty::S64
}

fn opt_int() -> Ty {
    Ty::Option(Box::new(Ty::S64))
}

/// A record type whose field names are kebab-case, as WAVE writes them, or the
/// snake_case the rows' `RetType`s are declared with.
fn record(snake: bool, fields: Vec<(&str, Ty)>) -> Ty {
    Ty::Record(
        fields
            .into_iter()
            .map(|(name, ty)| {
                let name = match snake {
                    true => name.replace('-', "_"),
                    false => String::from(name),
                };
                (name, ty)
            })
            .collect(),
    )
}

/// `text` as `build(snake)`'s type, in either spelling of the field names.
fn parse_either(text: &str, build: impl Fn(bool) -> Ty) -> Result<Val, String> {
    wave::parse(text, &build(false)).or_else(|_| wave::parse(text, &build(true)))
}

fn field(v: &Val, at: usize) -> Option<&Val> {
    match v {
        Val::Record(fields) => fields.get(at),
        _ => None,
    }
}

fn number(v: Option<&Val>) -> Option<u64> {
    match v? {
        Val::S64(n) => u64::try_from(*n).ok(),
        Val::Option(Some(inner)) => number(Some(inner)),
        _ => None,
    }
}

/// `cpus`: a count.
pub fn parse_cpus(text: &str) -> Result<u32, String> {
    number(Some(&wave::parse(text, &int())?))
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n > 0)
        .ok_or_else(|| String::from("cpus is not a count"))
}

/// `uptime_ms`: milliseconds.
pub fn parse_uptime(text: &str) -> Result<u64, String> {
    number(Some(&wave::parse(text, &int())?)).ok_or_else(|| String::from("uptime is not a number"))
}

/// `memory`: `{free_mib, total_mib, heap_kib}`.
pub fn parse_memory(text: &str) -> Result<Memory, String> {
    let v = parse_either(text, |snake| {
        record(
            snake,
            vec![
                ("free-mib", int()),
                ("total-mib", int()),
                ("heap-kib", int()),
            ],
        )
    })?;
    let get = |at| number(field(&v, at)).ok_or_else(|| String::from("memory has a bad number"));
    let memory = Memory {
        free_mib: get(0)?,
        total_mib: get(1)?,
        heap_kib: get(2)?,
    };
    match memory.free_mib <= memory.total_mib {
        true => Ok(memory),
        false => Err(String::from("memory has more free than total")),
    }
}

/// `processes`: a list of `{pid, name}`, and, when the OS gives them,
/// `cpu_permille` and `mem_kib`. At most [`MAX_PROCS`] are kept; one with a bad
/// pid is left out.
pub fn parse_processes(text: &str) -> Result<Vec<Proc>, String> {
    let list = |snake| {
        Ty::List(Box::new(record(
            snake,
            vec![
                ("pid", int()),
                ("name", Ty::String),
                ("cpu-permille", opt_int()),
                ("mem-kib", opt_int()),
            ],
        )))
    };
    let Val::List(items) = parse_either(text, list)? else {
        return Err(String::from("processes is not a list"));
    };
    Ok(items
        .iter()
        .take(MAX_PROCS)
        .filter_map(|v| {
            let Some(Val::String(name)) = field(v, 1) else {
                return None;
            };
            let shown = crate::tame(name, NAME_CHARS);
            Some(Proc {
                pid: u32::try_from(number(field(v, 0))?).ok()?,
                exact: shown == *name,
                name: shown,
                cpu_permille: number(field(v, 2)).and_then(|n| u32::try_from(n).ok()),
                mem_kib: number(field(v, 3)),
            })
        })
        .collect())
}

/// `model_state`: `{state, tokens_per_s_x10}`.
pub fn parse_model(text: &str) -> Result<Model, String> {
    let v = parse_either(text, |snake| {
        record(
            snake,
            vec![("state", Ty::String), ("tokens-per-s-x10", opt_int())],
        )
    })?;
    let Some(Val::String(state)) = field(&v, 0) else {
        return Err(String::from("model_state has no state"));
    };
    Ok(Model {
        state: crate::tame(state, 24),
        tokens_per_s_x10: number(field(&v, 1)).and_then(|n| u32::try_from(n).ok()),
    })
}
