//! A tagged call's arguments as consent reads them, and as Rhai literals.

use super::Arguments;
use agentabi::{Action, ArgType};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use llmproto::act::Call;
use serde_json::Value;

/// A string argument, or "" when the model left it out.
pub(super) fn text_arg(call: &Call, name: &str) -> String {
    call.arguments
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A call's arguments in the row's order, checked against its types: as text for
/// consent and as Rhai literals for the one-line script.
pub(crate) fn arguments(row: &'static Action, call: &Call) -> Result<Arguments, String> {
    if let Some(extra) = call
        .arguments
        .keys()
        .find(|key| !row.args.iter().any(|arg| arg.name == key.as_str()))
    {
        return Err(format!("{} has no argument {extra:?}", row.name));
    }
    let mut args = Arguments {
        values: Vec::new(),
        rhai: Vec::new(),
    };
    for arg in row.args {
        // An optional argument left out, or given as null, is a shorter call.
        let given = call.arguments.get(arg.name).filter(|v| !v.is_null());
        let Some(value) = given else {
            if matches!(arg.ty, ArgType::Opt(_)) {
                continue;
            }
            return Err(format!("{} needs its {} argument", row.name, arg.name));
        };
        let (text, rhai) = literal(&arg.ty, value)
            .ok_or_else(|| format!("{} is not a valid {}", value, arg.name))?;
        args.values.push((arg.name, text));
        args.rhai.push(rhai);
    }
    Ok(args)
}

/// `value` as text and as a Rhai literal, if it has the type `ty` allows.
fn literal(ty: &ArgType, value: &Value) -> Option<(String, String)> {
    match (ty, value) {
        (ArgType::Opt(inner), value) => literal(inner, value),
        (ArgType::Int { min, max }, Value::Number(n)) => {
            let n = n.as_i64().filter(|n| (*min..=*max).contains(n))?;
            Some((n.to_string(), n.to_string()))
        }
        (ArgType::Enum(words), Value::String(s)) if words.contains(&s.as_str()) => {
            Some((s.clone(), quote(s)?))
        }
        (ArgType::Text { max }, Value::String(s)) if s.chars().count() <= usize::from(*max) => {
            Some((s.clone(), quote(s)?))
        }
        (ArgType::Path | ArgType::Host, Value::String(s)) => Some((s.clone(), quote(s)?)),
        _ => None,
    }
}

/// A Rhai string literal for plain one-line text, quoted as the canonical call
/// text the ONCE digest names (`termcore::once::quote`).
fn quote(s: &str) -> Option<String> {
    termcore::once::quote(s)
}

/// Where content a call was steered by came from, in words: a window's view,
/// a file, or an app's answer, named as people know the app.
pub(crate) fn read_from(source: &str) -> String {
    match source {
        "ui" => String::from("what a window shows"),
        "fs" => String::from("a file"),
        "web" => String::from("a web page"),
        "dns" => String::from("a name lookup"),
        agentabi::consent::consent_grant::SYSTEM => String::from("the system's own state"),
        app => format!("what {} answered", crate::apps::title(app)),
    }
}
