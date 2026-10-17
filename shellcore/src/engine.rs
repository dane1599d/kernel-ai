//! The bounded Rhai engine every line and every script runs in: limits, captured
//! output, and the time, interrupt and heap checks made every [`CHECK_EVERY`]
//! operations.

use crate::{CHECK_EVERY, Config, Host, show};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use core::cell::{Cell, RefCell};
use rhai::{Dynamic, Engine, EvalAltResult};

/// Why a run was stopped by the shell itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    TimeLimit,
    Interrupted,
    Memory,
}

/// State shared with an engine's callbacks for the run in progress.
pub(crate) struct Line {
    pub deadline_ns: Cell<u64>,
    /// Heap in use when the run started; the budget counts what it adds.
    pub heap_start: Cell<u64>,
    pub stopped: Cell<Option<Stop>>,
    pub output: RefCell<String>,
    pub dropped: Cell<usize>,
}

impl Line {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            deadline_ns: Cell::new(0),
            heap_start: Cell::new(0),
            stopped: Cell::new(None),
            output: RefCell::new(String::new()),
            dropped: Cell::new(0),
        })
    }

    /// Clears the output and starts the clock for a new run.
    pub fn begin(&self, host: &dyn Host, config: &Config) {
        self.output.borrow_mut().clear();
        self.dropped.set(0);
        self.stopped.set(None);
        // A Ctrl-C meant for an earlier run must not stop this one.
        let _ = host.interrupted();
        self.heap_start.set(host.heap_bytes());
        let budget_ns = config.line_budget_ms * 1_000_000;
        self.deadline_ns
            .set(host.now_ns().saturating_add(budget_ns));
    }

    /// Printed output of the run, with a note if some was dropped.
    pub fn take_output(&self) -> String {
        let mut text = core::mem::take(&mut *self.output.borrow_mut());
        let dropped = self.dropped.get();
        if dropped > 0 {
            text.push_str(&format!("(output truncated: {dropped} bytes dropped)\n"));
        }
        text
    }

    /// An error as the user sees it.
    pub fn describe(&self, error: &EvalAltResult, config: &Config) -> String {
        match (error, self.stopped.get()) {
            (EvalAltResult::ErrorTerminated(..), Some(Stop::TimeLimit)) => {
                format!("stopped: time limit ({} ms) reached", config.line_budget_ms)
            }
            (EvalAltResult::ErrorTerminated(..), Some(Stop::Interrupted)) => {
                "stopped: interrupted".to_string()
            }
            (EvalAltResult::ErrorTerminated(..), Some(Stop::Memory)) => format!(
                "stopped: memory budget ({} KiB) reached",
                config.heap_budget / 1024
            ),
            (EvalAltResult::ErrorTooManyOperations(..), _) => {
                "stopped: too many operations".to_string()
            }
            _ => show::truncate(&error.to_string(), 512),
        }
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

/// A Rhai engine with the standard packages, every limit set, output captured into
/// `line`, and the run stopped when `line`'s deadline, an interrupt or the heap
/// budget is reached.
pub(crate) fn bounded(line: &Rc<Line>, host: &Rc<dyn Host>, config: &Config) -> Engine {
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
    let capture = Rc::clone(line);
    let max_output = config.max_output;
    engine.on_print(move |text| capture_output(&capture, max_output, text));
    let capture = Rc::clone(line);
    engine.on_debug(move |text, _, _| capture_output(&capture, max_output, text));
    let watch = Rc::clone(line);
    let watch_host = Rc::clone(host);
    let heap_budget = config.heap_budget;
    engine.on_progress(move |operations| {
        if !operations.is_multiple_of(CHECK_EVERY) {
            return None;
        }
        let used = watch_host
            .heap_bytes()
            .saturating_sub(watch.heap_start.get());
        let stop = if watch_host.interrupted() {
            Some(Stop::Interrupted)
        } else if watch_host.now_ns() > watch.deadline_ns.get() {
            Some(Stop::TimeLimit)
        } else if used > heap_budget {
            Some(Stop::Memory)
        } else {
            None
        };
        stop.map(|stop| {
            watch.stopped.set(Some(stop));
            Dynamic::UNIT
        })
    });
    engine
}
