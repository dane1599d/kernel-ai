//! Kai's C programs at `c>` (ADR 0012 D5 and D8; P4 task 14): `? <request>`
//! asks the model for a Kai C program with the `c-1` card
//! (`llmproto::program`, a script request whose `lang` is C).
//!
//! - **Checked first**: `llmproto::precheck` adds a missing `<string.h>`, and
//!   refuses a variable named after a call, a call declared again, or a
//!   `Print` that does not fit its values.
//! - **A trial on stub imports**: the program runs in a fresh model session
//!   of its own, and each `need` is answered with a stand-in value of the
//!   row's type. Nothing reaches the OS, and nothing is logged.
//! - **Repairs**: a refused or failed program goes back to the model with
//!   exactly why: the check's problems, wcc's message (the entry's line
//!   numbers) or the trial's trap. That happens at most [`MAX_REPAIRS`] times.
//! - **The run**: a program that passed its trial runs once more in a fresh
//!   model session. Its calls go through the one door with their own tiers,
//!   as a typed entry's do, and join one log entry, which records the
//!   program. Then cc brings the user's session back as it was (`back`).
//! - **`:keep`**: shows the last program's definitions and copies them into
//!   the user's session, where they replay after a restart like the user's
//!   own.

use super::{MAX_CALLS, ModelRun, Ran, resume};
use crate::{Reply, SCRIPT_ROUND_MS, Shell};
use agentabi::{ArgType, RetType};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use llmproto::script::{AnswerError, Lang, ScriptRequest};
use promptcore::wire::{Origin, Record, Request, parse_reply};
use serde_json::{Map, Value};

/// Repairs after the first answer: S12-2's re-run passed with this many.
pub const MAX_REPAIRS: usize = 3;

/// Items in a stand-in list: a list's full length on a small system, so an
/// index that works on the real one works in the trial too.
const STUB_ITEMS: usize = 8;

/// A program cc compiled or the trial ran, kept for `:keep`.
#[derive(Clone, Debug)]
pub(crate) struct Kept {
    pub(crate) program: String,
    pub(crate) names: Vec<String>,
}

impl Shell {
    /// `? <request>` at `c>` (see the module header).
    pub(super) fn c_program(&mut self, request: &str) -> Reply {
        if request.is_empty() {
            return Reply::err("? asks Kai for a C program: ? <what it should do>");
        }
        let calls: Vec<&str> = agentabi::SCRIPT_API.iter().map(|row| row.name).collect();
        let mut repair: Option<(String, String)> = None;
        for round in 0..=MAX_REPAIRS {
            let ask = ScriptRequest {
                request: request.to_string(),
                repair: repair.take(),
                timeout_ms: SCRIPT_ROUND_MS,
                lang: Lang::C,
            };
            let answer = match self.host.script(&ask) {
                Ok(answer) => answer,
                // No program needed: the summary is the answer, as for Rhai.
                Err(AnswerError::NoScript(summary)) if !summary.is_empty() => {
                    return Reply::ok(summary);
                }
                Err(error) if error.repairable() && round < MAX_REPAIRS => {
                    repair = Some((String::new(), error.describe()));
                    continue;
                }
                Err(error) => return Reply::err(error.describe()),
            };
            if answer.script.trim().is_empty() {
                return Reply::ok(answer.summary);
            }
            let prepared = llmproto::precheck::prepare(&answer.script, &calls);
            let why = match prepared.problems.is_empty() {
                false => format!(
                    "That program was refused before compiling:\n{}",
                    prepared.problems.join("\n")
                ),
                true => match self.c_trial(&prepared.source) {
                    Ok(()) => return self.c_model_run(request, &answer.summary, prepared.source),
                    Err(why) => why,
                },
            };
            if round == MAX_REPAIRS {
                return Reply::err(format!(
                    "Kai's program did not work after {MAX_REPAIRS} repairs:\n{why}"
                ));
            }
            repair = Some((answer.script, why));
        }
        Reply::err("Kai wrote no program")
    }

    /// The trial: `program` in a fresh model session, its `need`s answered by
    /// stand-ins. `Err` is why, for the repair.
    fn c_trial(&mut self, program: &str) -> Result<(), String> {
        self.c_open_model()?;
        let got = self.c_trial_run(program);
        self.c_back();
        got
    }

    fn c_trial_run(&mut self, program: &str) -> Result<(), String> {
        self.c.seq += 1;
        let eval = Request::Eval {
            seq: self.c.seq,
            entry: program,
        };
        let mut reply = self.host.cc(eval.text().as_bytes())?;
        for _ in 0..=MAX_CALLS {
            let records = parse_reply(&reply).map_err(|e| format!("cc's reply: {e}"))?;
            let mut need = None;
            for record in &records {
                match *record {
                    Record::Done { .. } => return Ok(()),
                    Record::Err { text, .. } => {
                        let text = String::from_utf8_lossy(text);
                        return Err(format!("wcc refused that program:\n{}", text.trim_end()));
                    }
                    Record::Trap { reason, function } => {
                        let at = function.map(|f| format!(" in {f}")).unwrap_or_default();
                        return Err(format!(
                            "That program stopped in its trial run: {reason}{at}"
                        ));
                    }
                    Record::Need { id, call } => need = Some((id, stub_answer(call))),
                    _ => {}
                }
            }
            let Some((id, value)) = need else {
                return Err(String::from("cc's reply did not end the trial"));
            };
            reply = self.host.cc(resume(id, value).as_bytes())?;
        }
        Err(format!(
            "That program made more than {MAX_CALLS} kai calls in its trial run"
        ))
    }

    /// The run for real (see the module header).
    fn c_model_run(&mut self, request: &str, summary: &str, program: String) -> Reply {
        if let Err(why) = self.c_open_model() {
            return Reply::err(why);
        }
        self.c.seq += 1;
        self.c.calls = 0;
        let seq = self.c.seq;
        self.entry_begin(&format!("? {request}"));
        if let Some(entry) = self.c_entry.as_mut() {
            entry.program = Some(program.clone());
        }
        self.c.model = Some(ModelRun {
            program: program.clone(),
            names: Vec::new(),
        });
        let shown = format!("kai: {summary}\n");
        let eval = Request::Eval {
            seq,
            entry: &program,
        };
        match self.host.cc(eval.text().as_bytes()) {
            Ok(reply) => self.c_drive(seq, &program, reply, shown),
            Err(why) => self.c_failed(&shown, &format!("cc did not answer: {why}")),
        }
    }

    /// A fresh model session; the user's is set aside.
    fn c_open_model(&mut self) -> Result<(), String> {
        let open = Request::Open {
            session: "kai",
            origin: Origin::Model,
        };
        let reply = self.host.cc(open.text().as_bytes())?;
        match parse_reply(&reply).map_err(|e| e.to_string())?.as_slice() {
            [Record::Ok] => Ok(()),
            [.., Record::Err { text, .. }] => Err(String::from_utf8_lossy(text).into_owned()),
            _ => Err(String::from("cc's reply was not ok")),
        }
    }

    /// Back to the user's session. If cc cannot, the next entry opens one and
    /// replays.
    pub(super) fn c_back(&mut self) {
        let back = self.host.cc(Request::Back.text().as_bytes());
        let ok = matches!(back.as_deref().map(parse_reply), Ok(Ok(ref r)) if matches!(r.as_slice(), [Record::Ok]));
        if !ok {
            self.c.open = false;
        }
    }

    /// Ends a model run, ok or not: back to the user's session, and what it
    /// defined is kept for `:keep`.
    pub(super) fn c_model_end(&mut self, ok: bool) {
        let Some(run) = self.c.model.take() else {
            return;
        };
        self.c_back();
        if ok && !run.names.is_empty() {
            self.c.kept = Some(Kept {
                program: run.program,
                names: run.names,
            });
        }
    }

    /// `:keep` (see the module header).
    pub(super) fn c_keep(&mut self) -> Reply {
        let Some(kept) = self.c.kept.clone() else {
            return Reply::err("nothing to keep: Kai's last program defined nothing");
        };
        let restored = match self.c_ensure_open() {
            Ok(note) => note,
            Err(why) => return Reply::err(why),
        };
        self.c.seq += 1;
        let seq = self.c.seq;
        let replay = Request::Replay {
            seq,
            entry: &kept.program,
        };
        let done = match self.host.cc(replay.text().as_bytes()) {
            Ok(reply) => matches!(
                parse_reply(&reply).as_deref(),
                Ok([.., Record::Done { .. }])
            ),
            Err(_) => false,
        };
        if !done {
            return Reply::err(format!("{restored}cc did not keep Kai's definitions"));
        }
        self.c.kept = None;
        self.c.ran.push(Ran {
            seq,
            defined: Some(kept.program.clone()),
            logged: None,
            cc: true,
        });
        Reply::ok(format!(
            "{restored}kept {}:\n{}",
            kept.names.join(", "),
            kept.program.trim_end()
        ))
    }
}

/// A stand-in answer for a trial's `need`: a value of the row's type, never an
/// effect. A call that is not a kai row is refused, as in a real run.
fn stub_answer(call: &[u8]) -> Result<Value, String> {
    let call = core::str::from_utf8(call)
        .ok()
        .and_then(|json| llmproto::act::parse(json).ok())
        .ok_or_else(|| String::from("cc sent a malformed call"))?;
    if !crate::centry::kai_row(&call.name) {
        return Err(format!("refused: {} is not a kai call", call.name));
    }
    let row = agentabi::find(&agentabi::ROWS, &call.name)
        .ok_or_else(|| format!("refused: {} is not a kai call", call.name))?;
    Ok(stub(&row.ret))
}

/// A value of type `ret`: numbers are non-zero, text is as long as its type
/// allows and lists are [`STUB_ITEMS`] long, so what works here can work on
/// the real system.
fn stub(ret: &RetType) -> Value {
    match ret {
        RetType::Unit => Value::Null,
        RetType::Data => Value::from("x"),
        RetType::Of(ty) => stub_scalar(ty),
        RetType::Record(fields) => {
            let mut out = Map::new();
            for (name, ty) in *fields {
                out.insert((*name).to_string(), stub(ty));
            }
            Value::Object(out)
        }
        RetType::List(item) => Value::Array((0..STUB_ITEMS).map(|_| stub(item)).collect()),
    }
}

fn stub_scalar(ty: &ArgType) -> Value {
    match ty {
        ArgType::Int { min, max } => Value::from(2i64.clamp(*min, *max)),
        ArgType::Enum(words) => Value::from(words.first().copied().unwrap_or("")),
        ArgType::Text { max } => Value::from("x".repeat(usize::from(*max).min(64))),
        _ => Value::from("x"),
    }
}
