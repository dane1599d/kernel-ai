//! `kai-call 1` and `kai-reply 1`, calls to an app's actions on the wire (ADR 0003
//! section 5):
//!
//! ```text
//! kai-call 1
//! func app:notes/actions@1.0.0#add
//! args {text: "buy milk"}
//! ```
//!
//! ```text
//! kai-reply 1
//! result ok(1)
//! ```
//!
//! `args` is a WAVE record of the export's parameters, by name; `result` is the
//! WAVE value it returned (absent for a function with no result). A call the
//! host cannot make (malformed, or naming nothing the app declares) is answered
//! `refused <why>` on one line instead. Effect lines
//! join the reply with the effect transaction. `ctl ` requests are unchanged.

use crate::actions::Actions;
use crate::{Fault, Instance};
use alloc::format;
use alloc::string::{String, ToString};
use appabi::{Val, wave};

pub const CALL_HEADER: &str = "kai-call 1";
pub const REPLY_HEADER: &str = "kai-reply 1";

/// A parsed call: the export and its arguments' WAVE text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub export: String,
    pub args: String,
}

/// Why a call was not answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// The request is malformed or names nothing the app declares: the caller's
    /// mistake, not the version's.
    Request(String),
    /// The version failed.
    Fault(Fault),
}

impl Call {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > kabi::MAX_MESSAGE {
            return Err("the call is too long".into());
        }
        let mut lines = text.lines();
        if lines.next() != Some(CALL_HEADER) {
            return Err("not a kai-call 1 request".into());
        }
        let export = lines
            .next()
            .and_then(|l| l.strip_prefix("func "))
            .filter(|e| !e.is_empty() && !e.contains(' '))
            .ok_or("expected func <export>")?
            .to_string();
        let args = lines
            .next()
            .and_then(|l| l.strip_prefix("args "))
            .ok_or("expected args <WAVE record>")?
            .to_string();
        if lines.any(|l| !l.trim().is_empty()) {
            return Err("unexpected lines after args".into());
        }
        Ok(Self { export, args })
    }

    pub fn format(&self) -> String {
        format!("{CALL_HEADER}\nfunc {}\nargs {}\n", self.export, self.args)
    }
}

/// A reply: the result's WAVE text, if the export returns one.
pub fn format_reply(result: Option<&str>) -> String {
    match result {
        Some(result) => format!("{REPLY_HEADER}\nresult {result}\n"),
        None => format!("{REPLY_HEADER}\n"),
    }
}

/// The reply to a call the host refused: the caller's mistake, on one line.
pub fn format_refusal(why: &str) -> String {
    let why: String = why
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!(
        "{REPLY_HEADER}
refused {why}
"
    )
}

/// The result text of a reply; a refusal is an error.
pub fn parse_reply(text: &str) -> Result<Option<String>, String> {
    let mut lines = text.lines();
    if lines.next() != Some(REPLY_HEADER) {
        return Err("not a kai-reply 1".into());
    }
    match lines.next() {
        None => Ok(None),
        Some(line) if line.starts_with("refused ") => Err(line.to_string()),
        Some(line) => line
            .strip_prefix("result ")
            .map(|r| Some(r.to_string()))
            .ok_or_else(|| format!("unexpected line {line:?}")),
    }
}

/// Answers a `kai-call 1` request on `instance`, typing it from `actions`.
pub fn serve(
    instance: &mut dyn Instance,
    actions: &Actions,
    request: &str,
) -> Result<String, CallError> {
    let call = Call::parse(request).map_err(CallError::Request)?;
    let row = actions.by_export(&call.export).ok_or_else(|| {
        CallError::Request(format!("{} is not an action of this app", call.export))
    })?;
    let args = match wave::parse(&call.args, &row.args_record()) {
        Ok(Val::Record(fields)) => fields,
        Ok(_) => unreachable!("a record type parses to a record"),
        Err(why) => return Err(CallError::Request(format!("args: {why}"))),
    };
    check_bounds(row, &args).map_err(CallError::Request)?;
    let value = instance.call(&row.func, &args).map_err(CallError::Fault)?;
    let text = match (&value, &row.func.result) {
        (Some(v), Some(ty)) => Some(wave::print(v, ty).map_err(|why| {
            CallError::Fault(Fault::new(crate::Reason::Abi, format!("result: {why}")))
        })?),
        _ => None,
    };
    Ok(format_reply(text.as_deref()))
}

/// The `max=` and `min=` of each argument: text length in bytes, integer value.
fn check_bounds(row: &crate::actions::ActionRow, args: &[Val]) -> Result<(), String> {
    for (arg, value) in row.args.iter().zip(args) {
        let n: Option<i128> = match value {
            Val::String(s) => Some(s.len() as i128),
            Val::U8(v) => Some((*v).into()),
            Val::U16(v) => Some((*v).into()),
            Val::U32(v) => Some((*v).into()),
            Val::U64(v) => Some((*v).into()),
            Val::S8(v) => Some((*v).into()),
            Val::S16(v) => Some((*v).into()),
            Val::S32(v) => Some((*v).into()),
            Val::S64(v) => Some((*v).into()),
            _ => None,
        };
        let Some(n) = n else { continue };
        if arg.max.is_some_and(|max| n > max.into()) || arg.min.is_some_and(|min| n < min.into()) {
            return Err(format!("{} is out of its bounds", arg.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions;
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    const ADD: &str = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";

    /// A notes stand-in: `add` returns `ok(<length of text>)`.
    struct Notes;

    impl Instance for Notes {
        fn schema(&mut self) -> Result<u32, Fault> {
            Ok(1)
        }
        fn init(&mut self) -> Result<u32, Fault> {
            Ok(0)
        }
        fn import(&mut self, _: &[u8], _: u32) -> Result<u32, Fault> {
            Ok(0)
        }
        fn export(&mut self, _: u32) -> Result<Option<Vec<u8>>, Fault> {
            Ok(None)
        }
        fn handle(&mut self, _: &[u8]) -> Result<Vec<u8>, Fault> {
            Ok(Vec::new())
        }
        fn health(&mut self) -> Result<u32, Fault> {
            Ok(0)
        }
        fn self_test(&mut self) -> Result<u32, Fault> {
            Ok(0)
        }
        fn quiet(&mut self, _: bool) {}
        fn call(&mut self, func: &appabi::Func, args: &[Val]) -> Result<Option<Val>, Fault> {
            match (func.export.as_str(), args) {
                ("app:notes/actions@1.0.0#add", [Val::String(text)]) => Ok(Some(Val::Result(Ok(
                    Some(Box::new(Val::U32(text.len() as u32))),
                )))),
                ("app:notes/actions@1.0.0#find", _) => Ok(Some(Val::List(Vec::new()))),
                _ => Err(Fault::new(crate::Reason::Trap, "unexpected")),
            }
        }
    }

    fn notes() -> Actions {
        actions::parse(actions::tests::NOTES).unwrap()
    }

    #[test]
    fn a_call_round_trips() {
        let call = Call::parse(ADD).unwrap();
        assert_eq!(call.export, "app:notes/actions@1.0.0#add");
        assert_eq!(call.format(), ADD);
        let reply = serve(&mut Notes, &notes(), ADD).unwrap();
        assert_eq!(reply, "kai-reply 1\nresult ok(8)\n");
        assert_eq!(parse_reply(&reply).unwrap().as_deref(), Some("ok(8)"));
        let find =
            "kai-call 1\nfunc app:notes/actions@1.0.0#find\nargs {word: \"milk\", order: newest}\n";
        assert_eq!(
            serve(&mut Notes, &notes(), find).unwrap(),
            "kai-reply 1\nresult []\n"
        );
    }

    #[test]
    fn bad_calls_are_the_callers() {
        let request = |e: Result<String, CallError>| matches!(e, Err(CallError::Request(_)));
        for bad in [
            "kai-call 2\nfunc x\nargs {}\n",
            "kai-call 1\nargs {}\n",
            "kai-call 1\nfunc app:notes/actions@1.0.0#delete\nargs {}\n",
            "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {txt: \"a\"}\n",
            "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: 3}\n",
            "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"a\"}\nmore\n",
        ] {
            assert!(request(serve(&mut Notes, &notes(), bad)), "{bad}");
        }
        let long = "x".repeat(2001);
        let over = alloc::format!(
            "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {{text: \"{long}\"}}\n"
        );
        assert!(request(serve(&mut Notes, &notes(), &over)));
    }

    #[test]
    fn a_refusal_is_one_line_and_reads_back_as_an_error() {
        let reply = format_refusal(
            "args: expected
a record",
        );
        assert_eq!(
            reply,
            "kai-reply 1
refused args: expected a record
"
        );
        assert_eq!(
            parse_reply(&reply),
            Err(String::from("refused args: expected a record"))
        );
    }
}
