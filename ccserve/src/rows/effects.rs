//! The rows as [`Effects`]: a call's arguments read from session memory into
//! ADR 0002's call JSON, and the shell's answer written back through the out
//! parameters.
//!
//! The answer is `resume <id> ok` with the row's value as JSON (a number for
//! an `Int`, a string for text and `Data`, an object for a record, an array
//! for a list, `null` for none), or `resume <id> err` with `refused …` or the
//! failure. Every pointer is checked before the call is asked about, so an
//! effect never runs for a result that could not be written.

use super::{
    KAI_E2BIG, KAI_EFAILED, KAI_EFAULT, KAI_EINVAL, KAI_EREFUSED, KAI_NONE, Lowered, Out, Scalar,
    declarations, lower,
};
use crate::Effects;
use agentabi::Action;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kailink::FuncType;
use serde_json::{Map, Value};

/// A result struct as the host writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// `kai_<row>` or `kai_<row>_item`.
    pub tag: String,
    pub size: u32,
    /// Each field's name and offset.
    pub fields: Vec<(&'static str, u32)>,
}

/// The rows session code may call.
pub struct Rows {
    lowered: Vec<Lowered>,
}

impl Rows {
    /// `rows` that lower, in order.
    pub fn new(rows: &[&'static Action]) -> Self {
        Self {
            lowered: rows.iter().filter_map(|row| lower(row)).collect(),
        }
    }

    /// The OS rows the one door runs (`agentcore::execute`): the bound rows,
    /// the file rows and the package rows, in the registry's order.
    pub fn os() -> Self {
        let runs = |row: &Action| {
            [
                &agentabi::BOUND[..],
                &agentabi::FS_ROWS,
                &agentabi::PKG_ROWS,
            ]
            .iter()
            .any(|rows| rows.iter().any(|r| r.name == row.name))
        };
        let rows: Vec<&'static Action> = agentabi::ROWS.iter().filter(|r| runs(r)).collect();
        Self::new(&rows)
    }

    /// The names of the rows lowered.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.lowered.iter().map(|l| l.row.name)
    }

    /// Each result struct as the host writes it. The tests compare these with
    /// the compiler's.
    pub fn layouts(&self) -> Vec<Layout> {
        let mut out = Vec::new();
        for lowered in &self.lowered {
            let (tag, fields, size) = match &lowered.out {
                Out::Record { fields, size } => {
                    (alloc::format!("kai_{}", lowered.row.name), fields, size)
                }
                Out::List { fields, size } => (
                    alloc::format!("kai_{}_item", lowered.row.name),
                    fields,
                    size,
                ),
                _ => continue,
            };
            let fields = fields.iter().map(|f| (f.name, f.offset)).collect();
            out.push(Layout {
                tag,
                size: *size,
                fields,
            });
        }
        out
    }

    fn find(&self, name: &str) -> Option<&Lowered> {
        self.lowered.iter().find(|l| l.row.name == name)
    }
}

impl Effects for Rows {
    fn imports(&self) -> Vec<(String, FuncType)> {
        self.lowered
            .iter()
            .map(|l| (l.row.name.to_string(), l.ty()))
            .collect()
    }

    fn declarations(&self) -> String {
        declarations(&self.lowered)
    }

    fn call(&self, name: &str, args: &[u64], memory: &[u8]) -> Result<String, i64> {
        let row = self.find(name).ok_or(KAI_EINVAL)?;
        let mut arguments = Map::new();
        for (param, &raw) in row.params.iter().zip(args) {
            if let Some(value) = argument(param.scalar, param.optional, raw, memory)? {
                arguments.insert(param.name.to_string(), value);
            }
        }
        check_out(
            &row.out,
            args.get(row.params.len()..).unwrap_or(&[]),
            memory,
        )?;
        let call = serde_json::json!({"name": row.row.name, "arguments": arguments});
        Ok(call.to_string())
    }

    fn answer(&self, name: &str, args: &[u64], ok: bool, result: &str, memory: &mut [u8]) -> i64 {
        let Some(row) = self.find(name) else {
            return KAI_EINVAL;
        };
        if !ok {
            return match result.starts_with("refused") {
                true => KAI_EREFUSED,
                false => KAI_EFAILED,
            };
        }
        let Ok(value) = serde_json::from_str::<Value>(result) else {
            return KAI_EFAILED;
        };
        let outs = args.get(row.params.len()..).unwrap_or(&[]);
        write_out(&row.out, outs, &value, memory).unwrap_or_else(|status| status)
    }
}

/// One argument as JSON; `None` for an optional one left out.
fn argument(scalar: Scalar, optional: bool, raw: u64, memory: &[u8]) -> Result<Option<Value>, i64> {
    match scalar {
        Scalar::Int { min, max } => {
            let value = raw as i64;
            if optional && value == KAI_NONE {
                return Ok(None);
            }
            match (min..=max).contains(&value) {
                true => Ok(Some(Value::from(value))),
                false => Err(KAI_EINVAL),
            }
        }
        Scalar::Text {
            bytes,
            chars,
            words,
        } => {
            let at = raw as u32 as usize;
            if at == 0 {
                return match optional {
                    true => Ok(None),
                    false => Err(KAI_EFAULT),
                };
            }
            let text = c_string(memory, at, bytes)?;
            if chars.is_some_and(|max| text.chars().count() > max) {
                return Err(KAI_E2BIG);
            }
            if words.is_some_and(|words| !words.contains(&text)) {
                return Err(KAI_EINVAL);
            }
            Ok(Some(Value::from(text)))
        }
    }
}

/// The NUL-terminated UTF-8 text at `at`, of at most `max` bytes.
fn c_string(memory: &[u8], at: usize, max: usize) -> Result<&str, i64> {
    let rest = memory.get(at..).ok_or(KAI_EFAULT)?;
    let window = &rest[..rest.len().min(max.saturating_add(1))];
    let Some(len) = window.iter().position(|&b| b == 0) else {
        // No NUL within the limit: too long, or memory ends first.
        return Err(match window.len() > max {
            true => KAI_E2BIG,
            false => KAI_EFAULT,
        });
    };
    core::str::from_utf8(&window[..len]).map_err(|_| KAI_EINVAL)
}

/// The out parameters: a pointer, and a count where there is one.
fn region(out: &Out, outs: &[u64]) -> Result<Option<(usize, usize)>, i64> {
    let ptr = |i: usize| outs.get(i).map(|&p| p as u32 as usize).ok_or(KAI_EFAULT);
    let cap = || {
        let cap = outs.get(1).map(|&c| c as i64).ok_or(KAI_EFAULT)?;
        usize::try_from(cap).map_err(|_| KAI_EINVAL)
    };
    Ok(match out {
        Out::Unit => None,
        Out::Int => Some((ptr(0)?, 8)),
        Out::Text => Some((ptr(0)?, cap()?)),
        Out::Record { size, .. } => Some((ptr(0)?, *size as usize)),
        Out::List { size, .. } => {
            let bytes = cap()?.checked_mul(*size as usize).ok_or(KAI_E2BIG)?;
            Some((ptr(0)?, bytes))
        }
    })
}

/// The out region is inside memory, and not at address 0 unless it is empty.
fn check_out(out: &Out, outs: &[u64], memory: &[u8]) -> Result<(), i64> {
    match region(out, outs)? {
        None | Some((_, 0)) => Ok(()),
        Some((0, _)) => Err(KAI_EFAULT),
        Some((at, len)) => match at.checked_add(len).is_some_and(|end| end <= memory.len()) {
            true => Ok(()),
            false => Err(KAI_EFAULT),
        },
    }
}

/// Writes the answer through the out parameters; the status.
fn write_out(out: &Out, outs: &[u64], value: &Value, memory: &mut [u8]) -> Result<i64, i64> {
    let Some((at, len)) = region(out, outs)? else {
        return Ok(0);
    };
    let end = at.checked_add(len).ok_or(KAI_EFAULT)?;
    let target = memory.get_mut(at..end).ok_or(KAI_EFAULT)?;
    match out {
        Out::Unit => Ok(0),
        Out::Int => {
            let n = value.as_i64().ok_or(KAI_EFAILED)?;
            target.copy_from_slice(&n.to_le_bytes());
            Ok(0)
        }
        Out::Text => {
            let text = value.as_str().ok_or(KAI_EFAILED)?;
            put_text(target, text);
            Ok(text.len() as i64)
        }
        Out::Record { fields, .. } => {
            let object = value.as_object().ok_or(KAI_EFAILED)?;
            put_fields(target, fields, |name| object.get(name))?;
            Ok(0)
        }
        Out::List { fields, size } => {
            let items = value.as_array().ok_or(KAI_EFAILED)?;
            let size = *size as usize;
            for (item, slot) in items.iter().zip(target.chunks_exact_mut(size)) {
                match item.as_object() {
                    Some(object) => put_fields(slot, fields, |name| object.get(name))?,
                    None => put_fields(slot, fields, |_| Some(item))?,
                }
            }
            Ok(items.len() as i64)
        }
    }
}

/// A struct's fields from `get`; a missing one is zero or empty.
fn put_fields<'v>(
    slot: &mut [u8],
    fields: &[super::Field],
    get: impl Fn(&str) -> Option<&'v Value>,
) -> Result<(), i64> {
    for field in fields {
        let at = field.offset as usize;
        let value = get(field.name);
        match field.text {
            None => {
                let n = match value {
                    Some(v) => v.as_i64().ok_or(KAI_EFAILED)?,
                    None => 0,
                };
                slot[at..at + 8].copy_from_slice(&n.to_le_bytes());
            }
            Some(bytes) => {
                let text = match value {
                    Some(v) => v.as_str().ok_or(KAI_EFAILED)?,
                    None => "",
                };
                put_text(&mut slot[at..at + bytes as usize], text);
            }
        }
    }
    Ok(())
}

/// `text` into `target` with its NUL, cut at a character boundary to fit.
fn put_text(target: &mut [u8], text: &str) {
    let Some(room) = target.len().checked_sub(1) else {
        return;
    };
    let mut cut = text.len().min(room);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    target[..cut].copy_from_slice(&text.as_bytes()[..cut]);
    target[cut] = 0;
}
