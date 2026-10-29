//! A `cc` service on the host: a clock that can be made to race, a Ctrl-C
//! flag, two stand-in effects, and replies written as short strings.

#![allow(dead_code)]

use ccserve::{Effects, Host, Service};
use kailink::{FuncType, ValType};
use promptcore::wire::{Record, parse_reply};
use std::cell::Cell;

pub struct Clock {
    /// Added to the time at every read, to make the deadline come at once.
    pub step_ns: Cell<u64>,
    now: Cell<u64>,
    pub interrupted: Cell<bool>,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            step_ns: Cell::new(0),
            now: Cell::new(0),
            interrupted: Cell::new(false),
        }
    }
}

impl Host for Clock {
    fn now_ns(&self) -> u64 {
        let real = std::time::Instant::now();
        let base = *BASE.get_or_init(|| real);
        self.now.set(self.now.get() + self.step_ns.get());
        real.duration_since(base).as_nanos() as u64 + self.now.get()
    }

    fn interrupted(&self) -> bool {
        self.interrupted.get()
    }
}

static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// `add_one(I64) -> I64` and `note(const char *, I32) -> I64`, whose calls
/// are written as ADR 0002's JSON.
pub struct Stand;

impl Effects for Stand {
    fn imports(&self) -> Vec<(String, FuncType)> {
        vec![
            (
                "add_one".into(),
                FuncType {
                    params: vec![ValType::I64],
                    results: vec![ValType::I64],
                },
            ),
            (
                "note".into(),
                FuncType {
                    params: vec![ValType::I32, ValType::I32],
                    results: vec![ValType::I64],
                },
            ),
        ]
    }

    fn call(&self, name: &str, args: &[u64], memory: &[u8]) -> Result<String, i64> {
        match name {
            "add_one" => Ok(format!(
                r#"{{"name":"add_one","arguments":{{"x":{}}}}}"#,
                args[0] as i64
            )),
            "note" => {
                let (at, len) = (args[0] as usize, args[1] as usize);
                let text = memory.get(at..at.checked_add(len).ok_or(-14)?).ok_or(-14)?;
                let text = std::str::from_utf8(text).map_err(|_| -22)?;
                Ok(format!(
                    r#"{{"name":"note","arguments":{{"text":"{text}"}}}}"#
                ))
            }
            _ => Err(-38),
        }
    }

    fn answer(&self, _: &str, _: &[u64], ok: bool, result: &str, _: &mut [u8]) -> i64 {
        match ok {
            true => result.parse().unwrap_or(-22),
            false => -1,
        }
    }
}

pub const DECLARE: &str = "__attribute__((import_module(\"kai\"), import_name(\"add_one\"))) I64 add_one(I64 x);\n\
__attribute__((import_module(\"kai\"), import_name(\"note\"))) I64 note(const char *text, I32 len);";

pub struct Cc {
    pub service: Service<Stand>,
    pub clock: Clock,
    seq: u64,
}

impl Cc {
    /// A service with a session open.
    pub fn open() -> Self {
        let clock = Clock::new();
        let service = Service::new(ccserve::CC_WASM, &ccserve::XCC_HEADERS, Stand, &clock)
            .expect("cc.wasm loads and kairt compiles");
        let mut cc = Self {
            service,
            clock,
            seq: 0,
        };
        assert_eq!(cc.send("open main origin=user"), ["ok"]);
        cc
    }

    /// One message, its reply as strings.
    pub fn send(&mut self, message: &str) -> Vec<String> {
        let reply = self.service.handle(message.as_bytes(), &self.clock);
        let records = parse_reply(&reply).unwrap_or_else(|e| panic!("{message}: {e}"));
        records.iter().map(show).collect()
    }

    /// Evaluates `entry` as the next entry.
    pub fn eval(&mut self, entry: &str) -> Vec<String> {
        self.seq += 1;
        self.send(&format!("eval {}\n{entry}", self.seq))
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Evaluates `entry`, which must finish, and returns its records but `done`.
    pub fn ok(&mut self, entry: &str) -> Vec<String> {
        let mut records = self.eval(entry);
        assert_eq!(
            records.last().map(String::as_str),
            Some("done"),
            "{entry}: {records:?}"
        );
        records.pop();
        records
    }

    /// The value `entry` shows.
    pub fn value(&mut self, entry: &str) -> String {
        let records = self.ok(entry);
        records
            .iter()
            .find_map(|r| r.strip_prefix("value "))
            .unwrap_or_else(|| panic!("{entry}: no value in {records:?}"))
            .to_string()
    }
}

pub fn show(record: &Record) -> String {
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    match record {
        Record::Out(b) => format!("out {}", text(b)),
        Record::Def { kind, slot, name } => match slot {
            Some(slot) => format!("def {} {name} {slot}", kind.word()),
            None => format!("def {} {name}", kind.word()),
        },
        Record::Retyped(name) => format!("retyped {name}"),
        Record::Value { ty, text: t } => format!("value {} {}", ty.word(), text(t)),
        Record::Need { id, call } => format!("need {id} {}", text(call)),
        Record::Done { .. } => "done".into(),
        Record::Err { line, text: t } => format!("err {line} {}", text(t)),
        Record::Trap { reason, .. } => format!("trap {reason}"),
        Record::Undone(seq) => format!("undone {seq}"),
        Record::Text(t) => format!("text {}", text(t)),
        Record::Ok => "ok".into(),
    }
}
