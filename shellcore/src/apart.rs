//! A checked script run in a process of its own (P1 task 14, ADR 0001 D6): the
//! shell claims a pooled `scriptrun` child, hands it the script and only the
//! run-tier handles its calls need, and reads back what it printed and changed.
//! The child checks the script again and refuses one whose calls are not the ones
//! the shell granted, so a request can do no more there than it was shown doing.
//!
//! The wire is text, every free field length-prefixed so no script or output
//! can forge one:
//!
//! ```text
//! script 1 <granted names, space-separated>
//! <source>
//! ```
//!
//! and back, `ran 1 ok=1 <n changes> <text len>`, the text, then per change
//! `<action> <owner> <res> <inverse>` with `before` and `after` length-prefixed
//! on the lines after it.

use crate::script::{self, Checked, Ran};
use crate::undo::{self, Done, Inverse};
use crate::{Config, Host};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use llmproto::script::Answer;

const REQUEST: &str = "script 1";
const RESULT: &str = "ran 1";

/// Whether `checked` may run apart: its calls are all run-tier rows a child can
/// make with the handles it is given. Ask-tier rows (the stops) stay in the shell
/// until the child can show the kernel their ONCE (task 14, part B), and so do
/// file rows: the child holds no `Dir`, and `fs` keys an open action by the
/// caller's process (P2 task 11). So do the window rows: `desktop` answers the
/// shell alone, over the shell's route (P3 task 10).
pub fn may_run_apart(checked: &Checked) -> bool {
    !checked.script.is_empty()
        && checked.plan.names().iter().all(|name| {
            !agentabi::CALL_ONLY.iter().any(|row| row.name == *name)
                && !name.starts_with("fs_")
                && !agentabi::UI_ROWS.iter().any(|row| row.name == *name)
                // An app's row is the shell's to call: its rows and the
                // supervisor's route are here, not in the child.
                && crate::apps::is_os_row(name)
        })
}

/// The request a child is sent for `checked`.
pub fn request(checked: &Checked) -> String {
    format!(
        "{REQUEST} {}\n{}",
        checked.plan.names().join(" "),
        checked.script
    )
}

/// The child's half: checks `request`'s script again, runs it with `host` (which
/// holds only what was handed over) and says what happened, as [`decode`] reads
/// it. Each call starts from nothing: a fresh engine, scope and journal.
pub fn serve(request: &str, host: &Rc<dyn Host>, config: &Config) -> String {
    let refused = |why: &str| {
        encode(&Ran {
            ok: false,
            text: why.to_string(),
            done: Vec::new(),
            value: None,
        })
    };
    let Some((head, source)) = request.split_once('\n') else {
        return refused("the script's request was malformed");
    };
    let Some(granted) = head.strip_prefix(REQUEST) else {
        return refused("the script's request was malformed");
    };
    let granted: Vec<&str> = granted.split_whitespace().collect();
    let answer = Answer {
        summary: String::new(),
        script: source.to_string(),
    };
    let checked = match script::check_call(&answer, host, config) {
        Ok(checked) => checked,
        Err(refusal) => return refused(&format!("refused here: {refusal}")),
    };
    if let Some(extra) = checked.plan.names().iter().find(|n| !granted.contains(n)) {
        return refused(&format!("refused here: {extra} was not granted"));
    }
    encode(&script::run(&checked, host, config))
}

/// What a run says, for the shell.
pub fn encode(ran: &Ran) -> String {
    let mut out = format!(
        "{RESULT} ok={} {} {}\n{}\n",
        u8::from(ran.ok),
        ran.done.len(),
        ran.text.len(),
        ran.text
    );
    for done in &ran.done {
        let _ = writeln!(
            out,
            "{} {} {} {}\n{} {}\n{} {}",
            word(&done.action),
            word(&done.owner),
            word(&done.res),
            undo::inverse_word(done.inverse),
            done.before.len(),
            done.before,
            done.after.len(),
            done.after
        );
    }
    out
}

/// A child's answer, or `None` for text it could not have written.
pub fn decode(text: &str) -> Option<Ran> {
    let (head, mut rest) = text.split_once('\n')?;
    let mut words = head.strip_prefix(RESULT)?.split_whitespace();
    let ok = match words.next()? {
        "ok=1" => true,
        "ok=0" => false,
        _ => return None,
    };
    let count: usize = words.next()?.parse().ok()?;
    let len: usize = words.next()?.parse().ok()?;
    let printed = take(&mut rest, len)?;
    let mut done = Vec::new();
    for _ in 0..count.min(64) {
        let (line, after) = rest.split_once('\n')?;
        rest = after;
        let mut fields = line.split(' ');
        let (action, owner, res) = (fields.next()?, fields.next()?, fields.next()?);
        let inverse = undo::parse_inverse(fields.next()?)?;
        let before = sized(&mut rest)?;
        let after = sized(&mut rest)?;
        done.push(Done {
            action: action.to_string(),
            owner: owner.to_string(),
            res: res.to_string(),
            before,
            after,
            inverse,
        });
    }
    Some(Ran {
        ok,
        text: printed,
        done,
        value: None,
    })
}

/// What a script killed for time or Ctrl-C changed, read from what runs now
/// against `before`, the spec taken before its claim: its journal never came
/// back, so the policy is the one change the shell can see and put back.
pub fn killed(why: &str, before: Option<(u64, u64)>, host: &dyn Host) -> Ran {
    let now = undo::current_spec(host);
    let done = match (before, now) {
        (Some((policy, preset)), Some(after)) if after != (policy, preset) => {
            alloc::vec![Done {
                action: "set_policy".into(),
                owner: "kernel".into(),
                res: "policy".into(),
                before: undo::spec_name(policy, preset),
                after: undo::spec_name(after.0, after.1),
                inverse: Some(Inverse::Spec {
                    policy,
                    preset,
                    after,
                }),
            }]
        }
        _ => Vec::new(),
    };
    Ran {
        ok: false,
        text: why.to_string(),
        done,
        value: None,
    }
}

/// `n text`, then the newline after it.
fn take(rest: &mut &str, len: usize) -> Option<String> {
    let text = rest.get(..len)?;
    *rest = rest.get(len..)?.strip_prefix('\n')?;
    Some(text.to_string())
}

/// A `len text` line.
fn sized(rest: &mut &str) -> Option<String> {
    let (len, after) = rest.split_once(' ')?;
    *rest = after;
    take(rest, len.parse().ok()?)
}

/// A field as one word: registry names and owners have no spaces.
fn word(text: &str) -> &str {
    match text.is_empty() || text.contains([' ', '\n']) {
        true => "-",
        false => text,
    }
}
