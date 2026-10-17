//! The Rhai shell, kept free of system calls so it can be tested on the host. The
//! ring-3 `shell` program supplies a [`Host`] and feeds it one line at a time.
//!
//! Like TempleOS's prompt, every line is code: variables and functions persist, and
//! the value of the last expression is echoed. Plain commands (`mem`, `policy rr`,
//! `ask why is the sky blue`) are recognised from their first word and run directly.
//!
//! Every line is bounded: by time (checked every [`CHECK_EVERY`] operations), by an
//! operation count, by heap use, and by Rhai's own size and depth limits. A line that
//! hits a bound returns an error and leaves the shell running.

#![no_std]

extern crate alloc;

mod commands;
mod format;

use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};

pub use commands::{COMMANDS, split_command};

/// What the shell needs from the operating system.
pub trait Host {
    fn now_ns(&self) -> u64;
    /// True if the user pressed Ctrl-C since the last call; reading clears it.
    fn interrupted(&self) -> bool;
    /// Bytes of heap in use.
    fn heap_bytes(&self) -> u64;
    fn metrics(&self) -> Result<kabi::Metrics, kabi::Error>;
    fn set_policy(&self, id: u64) -> Result<(), kabi::Error>;
    /// Asks the LLM; its reply, or why there is none.
    fn ask(&self, prompt: &str) -> Result<String, String>;
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Longest a line may compute, not counting time spent waiting in `ask`.
    pub line_budget_ms: u64,
    /// Heap use at which a line is stopped.
    pub heap_budget: u64,
    /// Most bytes of printed output kept per line.
    pub max_output: usize,
    /// Backstop if the clock check never fires.
    pub max_operations: u64,
}

pub const DEFAULT_CONFIG: Config = Config {
    line_budget_ms: 2000,
    heap_budget: 8 * 1024 * 1024,
    max_output: 6 * 1024,
    max_operations: 20_000_000,
};

/// How often the time, interrupt and heap checks run, in operations.
pub const CHECK_EVERY: u64 = 1024;

/// The shell's answer to one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub ok: bool,
    pub text: String,
}

impl Reply {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
        }
    }

    /// `ok <text>` or `err <text>`, as sent back to the terminal.
    pub fn to_wire(&self) -> String {
        format!("{} {}", if self.ok { "ok" } else { "err" }, self.text)
    }
}

/// Why a line was stopped by the shell itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    TimeLimit,
    Interrupted,
    Memory,
}

/// State shared with the engine's callbacks.
struct Line {
    deadline_ns: Cell<u64>,
    stopped: Cell<Option<Stop>>,
    output: RefCell<String>,
    dropped: Cell<usize>,
}

pub struct Shell {
    engine: Engine,
    scope: Scope<'static>,
    /// Every function defined so far, and no statements.
    functions: AST,
    line: Rc<Line>,
    host: Rc<dyn Host>,
    config: Config,
}

impl Shell {
    pub fn new(host: Rc<dyn Host>, config: Config) -> Self {
        let line = Rc::new(Line {
            deadline_ns: Cell::new(0),
            stopped: Cell::new(None),
            output: RefCell::new(String::new()),
            dropped: Cell::new(0),
        });
        let mut engine = Engine::new();
        engine
            .set_max_operations(config.max_operations)
            .set_max_call_levels(16)
            .set_max_expr_depths(32, 16)
            .set_max_string_size(64 * 1024)
            .set_max_array_size(1000)
            .set_max_map_size(1000)
            .set_max_variables(256)
            .set_max_functions(256);
        let capture = Rc::clone(&line);
        let max_output = config.max_output;
        engine.on_print(move |text| capture_output(&capture, max_output, text));
        let capture = Rc::clone(&line);
        engine.on_debug(move |text, _, _| capture_output(&capture, max_output, text));
        let watch = Rc::clone(&line);
        let watch_host = Rc::clone(&host);
        let heap_budget = config.heap_budget;
        engine.on_progress(move |operations| {
            if !operations.is_multiple_of(CHECK_EVERY) {
                return None;
            }
            let stop = if watch_host.interrupted() {
                Some(Stop::Interrupted)
            } else if watch_host.now_ns() > watch.deadline_ns.get() {
                Some(Stop::TimeLimit)
            } else if watch_host.heap_bytes() > heap_budget {
                Some(Stop::Memory)
            } else {
                None
            };
            stop.map(|stop| {
                watch.stopped.set(Some(stop));
                Dynamic::UNIT
            })
        });
        commands::register(&mut engine, &host, &line, config.line_budget_ms);
        Self {
            engine,
            scope: Scope::new(),
            functions: AST::empty(),
            line,
            host,
            config,
        }
    }

    /// Runs one line and returns what to show.
    pub fn eval(&mut self, input: &str) -> Reply {
        let input = input.trim();
        self.line.output.borrow_mut().clear();
        self.line.dropped.set(0);
        self.line.stopped.set(None);
        // A Ctrl-C meant for an earlier line must not stop this one.
        let _ = self.host.interrupted();
        let budget_ns = self.config.line_budget_ms * 1_000_000;
        self.line
            .deadline_ns
            .set(self.host.now_ns().saturating_add(budget_ns));
        if input.is_empty() {
            return Reply::ok("");
        }
        if input == "reset" {
            self.scope.clear();
            self.functions = AST::empty();
            return Reply::ok("variables and functions cleared");
        }
        if let Some((name, rest)) = split_command(input, |name| self.scope.contains(name)) {
            return match commands::run(self.host.as_ref(), name, rest) {
                Ok(text) => Reply::ok(text),
                Err(error) => Reply::err(error),
            };
        }
        let ast = match self.engine.compile_with_scope(&self.scope, input) {
            Ok(ast) => ast,
            Err(error) => return Reply::err(format!("syntax error: {error}")),
        };
        let program = self.functions.merge(&ast);
        let result = self
            .engine
            .eval_ast_with_scope::<Dynamic>(&mut self.scope, &program);
        self.compact_scope();
        let mut text = self.take_output();
        match result {
            Ok(value) => {
                self.functions = self.functions.merge(&ast.clone_functions_only());
                if !value.is_unit() {
                    text.push_str(&format::value(&value));
                }
                Reply::ok(text)
            }
            Err(error) => {
                text.push_str(&self.describe(&error));
                Reply::err(text)
            }
        }
    }

    fn describe(&self, error: &EvalAltResult) -> String {
        match (error, self.line.stopped.get()) {
            (EvalAltResult::ErrorTerminated(..), Some(Stop::TimeLimit)) => format!(
                "stopped: time limit ({} ms) reached",
                self.config.line_budget_ms
            ),
            (EvalAltResult::ErrorTerminated(..), Some(Stop::Interrupted)) => {
                "stopped: interrupted".to_string()
            }
            (EvalAltResult::ErrorTerminated(..), Some(Stop::Memory)) => format!(
                "stopped: memory budget ({} KiB) reached",
                self.config.heap_budget / 1024
            ),
            (EvalAltResult::ErrorTooManyOperations(..), _) => {
                "stopped: too many operations".to_string()
            }
            _ => format::truncate(&error.to_string(), 512),
        }
    }

    /// Printed output of the line, with a note if some was dropped.
    fn take_output(&self) -> String {
        let mut text = core::mem::take(&mut *self.line.output.borrow_mut());
        let dropped = self.line.dropped.get();
        if dropped > 0 {
            text.push_str(&format!("(output truncated: {dropped} bytes dropped)\n"));
        }
        text
    }

    /// Every `let` adds a scope entry, even for a name that exists: keep only the
    /// newest of each, so the variable limit counts names, not assignments.
    fn compact_scope(&mut self) {
        let entries: Vec<(String, bool, Dynamic)> = self
            .scope
            .iter()
            .map(|(name, constant, value)| (name.to_string(), constant, value))
            .collect();
        if entries.len() < 2 {
            return;
        }
        let mut scope = Scope::new();
        for (index, (name, constant, value)) in entries.iter().enumerate() {
            if entries[index + 1..].iter().any(|(later, ..)| later == name) {
                continue;
            }
            if *constant {
                scope.push_constant_dynamic(name.clone(), value.clone());
            } else {
                scope.push_dynamic(name.clone(), value.clone());
            }
        }
        self.scope = scope;
    }

    /// How many variables and functions the shell holds.
    pub fn sizes(&self) -> (usize, usize) {
        (self.scope.len(), self.functions.iter_functions().count())
    }
}

fn capture_output(line: &Line, max: usize, text: &str) {
    let mut output = line.output.borrow_mut();
    if output.len() + text.len() < max {
        output.push_str(text);
        output.push('\n');
    } else {
        line.dropped.set(line.dropped.get() + text.len() + 1);
    }
}

/// A Rhai error from a message.
fn runtime_error(message: impl Into<String>) -> Box<EvalAltResult> {
    let message: String = message.into();
    message.into()
}

#[cfg(test)]
mod tests;
