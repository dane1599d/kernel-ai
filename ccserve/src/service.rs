//! One `kai-cc 1` request in, one reply out (ADR 0012 D3, D11).
//!
//! An entry is prepared by promptcore, compiled by cc.wasm, then linked and run
//! in the session. When its last expression shows a value and the unit does
//! not compile (a `void` call), the plain fallback unit is compiled instead.
//! wcc's messages come back with the entry's own line numbers.
//!
//! An effect suspends the run: the reply ends with `need`, and the run goes on
//! when `resume` brings the shell's answer. Any other request drops the
//! suspended run first, and nothing it defined stays. Definitions are committed
//! (promptcore's and kailink's together) only when an entry ran to its end, so
//! `undo` always reverts both journals by one entry.

use crate::compiler::CompileError;
use crate::dis::MAX_TEXT;
use crate::session::{Done, Failure, Step};
use crate::{Budget, Compiler, Host, Session};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use kailink::{FuncType, Symbol};
use promptcore::wire::{Origin, Record, Reply, Request};
use promptcore::{Defs, Kind, LineMap, Mode, Prepared};

/// Longest reply: kabi's `MAX_MESSAGE`, one IPC message.
pub const REPLY_CAP: usize = 8192;
/// Longest call a `need` record carries; an effect whose call is longer is
/// answered at once with [`E2BIG`].
pub const MAX_CALL: usize = 1024;
/// Room kept for the record that ends a reply: a `need` with its call, or a
/// shorter one. Output (at most 6 KiB) and the rest fit in what is left.
const END_ROOM: usize = MAX_CALL + 64;
/// The status an effect gets when its call is too long to ask about.
pub const E2BIG: i64 = -7;
/// Longest error text sent back.
const MAX_ERR: usize = 2048;

/// kai.h's fixed part, and kairt's source (D1, D2).
pub const KAI_H: &str = include_str!("../kai/kai.h");
pub const KAIRT: &str = include_str!("../kai/kairt.c");

/// How effect imports become ADR 0002 calls and back (D5, D6).
pub trait Effects {
    /// The `kai` imports that are effects, with their types.
    fn imports(&self) -> Vec<(String, FuncType)>;
    /// The call for effect `name` called with `args` (each as its bits),
    /// reading any text from `memory`: ADR 0002's call as JSON. `Err(status)`
    /// answers at once with that status, as for a pointer out of bounds.
    fn call(&self, name: &str, args: &[u64], memory: &[u8]) -> Result<String, i64>;
    /// The effect's result for the shell's answer, writing any text result
    /// into `memory`.
    fn answer(&self, name: &str, args: &[u64], ok: bool, result: &str, memory: &mut [u8]) -> i64;
    /// C declarations of the imports, added to kai.h.
    fn declarations(&self) -> String {
        String::new()
    }
}

/// kai.h: the fixed part and `effects`' declarations, in one include guard.
pub fn kai_h(effects: &dyn Effects) -> String {
    format!(
        "#ifndef KAI_H\n#define KAI_H\n{KAI_H}{}#endif\n",
        effects.declarations()
    )
}

pub struct Service<E> {
    compiler: Compiler,
    kairt: Vec<u8>,
    effects: E,
    open: Option<Open>,
}

struct Open {
    defs: Defs,
    session: Session,
    waiting: Option<Waiting>,
    /// The last `need` id given.
    needs: u64,
}

/// An entry suspended on an effect.
struct Waiting {
    /// The `need` record's id, which `resume` must name.
    id: u64,
    name: String,
    args: Vec<u64>,
    prepared: Prepared,
    compile_us: u64,
}

impl<E: Effects> Service<E> {
    /// Translates `cc_wasm` and compiles kairt with it; `headers` are xcc's,
    /// and kai.h is added to them.
    pub fn new(
        cc_wasm: &[u8],
        headers: &[(&str, &[u8])],
        effects: E,
        host: &dyn Host,
    ) -> Result<Self, String> {
        let kai_h = kai_h(&effects);
        let mut all: Vec<(&str, &[u8])> = headers.to_vec();
        all.push(("kai.h", kai_h.as_bytes()));
        let compiler = Compiler::new(cc_wasm, &all)?;
        let kairt = compiler
            .compile(
                KAIRT,
                &mut Budget::new(host, host.line_ns().saturating_mul(10)),
            )
            .map_err(|e| format!("kairt: {e:?}"))?;
        Ok(Self {
            compiler,
            kairt,
            effects,
            open: None,
        })
    }

    /// Answers one message.
    pub fn handle(&mut self, message: &[u8], host: &dyn Host) -> Vec<u8> {
        let mut reply = Reply::new(REPLY_CAP);
        let request = core::str::from_utf8(message)
            .map_err(|_| "a request is text")
            .and_then(|text| Request::parse(text).map_err(|e| e.0));
        match request {
            Ok(request) => self.serve(request, host, &mut reply),
            Err(why) => error(&mut reply, 0, why),
        }
        reply.into_bytes()
    }

    fn serve(&mut self, request: Request, host: &dyn Host, reply: &mut Reply) {
        if let Request::Open { origin, .. } = request {
            return self.open(origin, reply);
        }
        let Some(open) = self.open.as_mut() else {
            return error(reply, 0, "no session is open");
        };
        if !matches!(request, Request::Resume { .. }) && open.waiting.take().is_some() {
            open.session.abort();
        }
        let effects = &self.effects;
        match request {
            Request::Open { .. } => {}
            Request::Eval { seq, entry } => eval(
                &self.compiler,
                effects,
                open,
                seq,
                entry,
                Mode::Eval,
                host,
                reply,
            ),
            Request::Replay { seq, entry } => eval(
                &self.compiler,
                effects,
                open,
                seq,
                entry,
                Mode::Replay,
                host,
                reply,
            ),
            Request::Resume { id, ok, result } => {
                resume(effects, open, id, ok, result, host, reply)
            }
            Request::Undo => match (open.session.undo(), open.defs.undo()) {
                (Some(seq), Some(_)) => end(reply, &Record::Undone(seq)),
                (None, None) => error(reply, 0, "nothing to undo at c>"),
                _ => error(reply, 0, "cc's journals disagree; :reset starts again"),
            },
            Request::Dis(name) => match open.session.dis(name) {
                Some(text) => end(reply, &Record::Text(text.as_bytes())),
                None => error(
                    reply,
                    0,
                    &format!("{name} is not a function of this session"),
                ),
            },
            Request::Defs => end(
                reply,
                &Record::Text(capped(&open.defs.listing()).as_bytes()),
            ),
            Request::Reset => self.open(Origin::User, reply),
            Request::Build(_) => error(reply, 0, "cc build comes with P4 task 13"),
        }
    }

    fn open(&mut self, origin: Origin, reply: &mut Reply) {
        if origin == Origin::Model {
            // A model's program runs in its own session after a trial run on
            // stub imports (D5); that is P4 task 14, after spike S12-2.
            return error(reply, 0, "model-written C is not enabled");
        }
        match Session::new(&self.kairt, self.effects.imports()) {
            Ok(session) => {
                self.open = Some(Open {
                    defs: Defs::new(),
                    session,
                    waiting: None,
                    needs: 0,
                });
                end(reply, &Record::Ok);
            }
            Err(why) => error(reply, 0, &why),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn eval(
    compiler: &Compiler,
    effects: &dyn Effects,
    open: &mut Open,
    seq: u64,
    entry: &str,
    mode: Mode,
    host: &dyn Host,
    reply: &mut Reply,
) {
    let prepared = match open.defs.prepare(seq, entry, mode) {
        Ok(prepared) => prepared,
        Err(e) => return error(reply, e.line(), &e.to_string()),
    };
    let started = host.now_ns();
    let object = match compile(compiler, &prepared, host) {
        Ok(object) => object,
        Err(CompileError::Source(text)) => {
            let (line, text) = wcc_messages(&text, &prepared.lines);
            return error(reply, line, &text);
        }
        Err(CompileError::Stopped(stop)) => return trap(reply, stop.word()),
        Err(CompileError::Compiler(why)) => return error(reply, 0, &format!("cc: {why}")),
    };
    let compile_us = host.now_ns().saturating_sub(started) / 1000;
    let run = prepared.run.as_deref();
    let step = open.session.start(seq, &object, run, host);
    finish(effects, open, step, prepared, compile_us, host, reply);
}

/// Compiles the unit, or its fallback if the unit does not compile.
fn compile(
    compiler: &Compiler,
    prepared: &Prepared,
    host: &dyn Host,
) -> Result<Vec<u8>, CompileError> {
    let budget = &mut Budget::new(host, host.line_ns());
    match (
        compiler.compile(&prepared.source, budget),
        &prepared.fallback,
    ) {
        (Err(CompileError::Source(_)), Some(fallback)) => compiler.compile(fallback, budget),
        (result, _) => result,
    }
}

fn resume(
    effects: &dyn Effects,
    open: &mut Open,
    id: u64,
    ok: bool,
    result: &str,
    host: &dyn Host,
    reply: &mut Reply,
) {
    let Some(waiting) = open.waiting.take() else {
        return error(reply, 0, "no run is waiting");
    };
    if waiting.id != id {
        open.session.abort();
        return error(reply, 0, "resume names another effect");
    }
    let memory = open.session.memory_mut();
    let value = effects.answer(&waiting.name, &waiting.args, ok, result, memory);
    let step = open.session.resume(value, host);
    finish(
        effects,
        open,
        step,
        waiting.prepared,
        waiting.compile_us,
        host,
        reply,
    );
}

/// Replies to what the session did: commits a finished entry, asks for an
/// effect, or reports a failure. Effects whose arguments are refused are
/// answered here and the run goes on (the session checks the budget on each
/// resume). The output printed meanwhile is sent once, in one record, before
/// the reply's end.
fn finish(
    effects: &dyn Effects,
    open: &mut Open,
    mut step: Result<Step, Failure>,
    prepared: Prepared,
    compile_us: u64,
    host: &dyn Host,
    reply: &mut Reply,
) {
    loop {
        let (name, args) = match step {
            Ok(Step::Need { name, args }) => (name, args),
            Ok(Step::Done(done)) => {
                output(open, reply);
                return committed(open, done, prepared, compile_us, reply);
            }
            Err(failure) => {
                output(open, reply);
                return failed(reply, failure);
            }
        };
        let call = match effects.call(&name, &args, open.session.memory()) {
            Ok(call) if call.len() > MAX_CALL => Err(E2BIG),
            other => other,
        };
        match call {
            Ok(call) => {
                open.needs += 1;
                let id = open.needs;
                output(open, reply);
                end(
                    reply,
                    &Record::Need {
                        id,
                        call: call.as_bytes(),
                    },
                );
                open.waiting = Some(Waiting {
                    id,
                    name,
                    args,
                    prepared,
                    compile_us,
                });
                return;
            }
            Err(status) => step = open.session.resume(status, host),
        }
    }
}

fn failed(reply: &mut Reply, failure: Failure) {
    match failure {
        Failure::Link(e) => error(reply, 0, &e.to_string()),
        Failure::Trap(reason) => trap(reply, reason),
        Failure::Stopped(stop) => trap(reply, stop.word()),
        Failure::Full => error(
            reply,
            0,
            "the session holds 512 entries; :reset starts again",
        ),
        Failure::Internal(why) => error(reply, 0, &format!("cc: {why}")),
    }
}

/// `:defs`' listing, cut at whole lines to fit a reply.
fn capped(listing: &str) -> String {
    if listing.len() <= MAX_TEXT {
        return listing.into();
    }
    let cut = listing[..MAX_TEXT].rfind('\n').map_or(0, |at| at + 1);
    let more = listing[cut..].lines().count();
    format!("{}… and {more} more\n", &listing[..cut])
}

fn committed(open: &mut Open, done: Done, prepared: Prepared, compile_us: u64, reply: &mut Reply) {
    for def in &prepared.defs {
        let slot = done
            .defs
            .iter()
            .find(|d| d.name == def.name)
            .and_then(|d| match d.symbol {
                Symbol::Func { slot, .. } if def.kind == Kind::Func => Some(slot),
                _ => None,
            });
        let record = Record::Def {
            kind: def.kind,
            slot,
            name: &def.name,
        };
        // Definitions past the reply's room are still made; `:defs` lists them.
        let _ = reply.push_leaving(&record, END_ROOM);
    }
    for name in &done.retyped {
        let _ = reply.push_leaving(&Record::Retyped(name), END_ROOM);
    }
    if let Some((ty, text)) = &done.value {
        let _ = reply.push_leaving(&Record::Value { ty: *ty, text }, END_ROOM);
    }
    open.defs
        .commit(prepared.seq, prepared.defs, &prepared.drops);
    end(
        reply,
        &Record::Done {
            compile_us,
            link_us: done.link_ns / 1000,
            run_us: done.run_ns / 1000,
        },
    );
}

/// The output printed so far, as much as fits.
fn output(open: &mut Open, reply: &mut Reply) {
    let note: &[u8] = b"\n[output cut at 6 KiB]\n";
    let (out, cut) = open.session.take_output();
    if !out.is_empty() {
        // Two record headers and the note fit after it.
        let room = reply.room().saturating_sub(END_ROOM + 2 * 16 + note.len());
        let _ = reply.push_leaving(&Record::Out(&out[..out.len().min(room)]), END_ROOM);
    }
    if cut {
        let _ = reply.push_leaving(&Record::Out(note), END_ROOM);
    }
}

fn end(reply: &mut Reply, record: &Record) {
    // The room kept for it always holds an ending record.
    let _ = reply.push(record);
}

fn error(reply: &mut Reply, line: u32, text: &str) {
    let mut cut = text.len().min(MAX_ERR);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    end(
        reply,
        &Record::Err {
            line,
            text: &text.as_bytes()[..cut],
        },
    );
}

fn trap(reply: &mut Reply, reason: &'static str) {
    end(
        reply,
        &Record::Trap {
            reason,
            function: None,
        },
    );
}

/// wcc's messages with the entry's line numbers: `/w/entry.c(12): x` becomes
/// `line 2: x`. wcc's own assertion lines are dropped. Returns the first
/// entry line named, or 0.
fn wcc_messages(stderr: &str, lines: &LineMap) -> (u32, String) {
    let mut first = 0;
    let mut out = String::new();
    for line in stderr.lines() {
        let located = line
            .strip_prefix("/w/entry.c(")
            .and_then(|rest| rest.split_once("): "))
            .and_then(|(n, message)| Some((n.parse::<u32>().ok()?, message)));
        match located {
            Some((n, message)) => match lines.entry_line(n) {
                Some(at) => {
                    if first == 0 {
                        first = at;
                    }
                    let _ = writeln!(out, "line {at}: {message}");
                }
                None => {
                    let _ = writeln!(out, "in the session's definitions: {message}");
                }
            },
            None if line.starts_with("src/") => {}
            None => {
                let _ = writeln!(out, "{line}");
            }
        }
    }
    (first, out)
}
