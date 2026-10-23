//! The bounded Rhai engine every line and every script runs in: limits (including a
//! size-checked `replace` and `pad`), captured output, the heap check on every
//! operation, and the time and interrupt checks made every [`CHECK_EVERY`] operations.
//! [`eval`] runs compiled code by walking its AST, or on Rhai's Grain bytecode VM when
//! the `grain` feature is on and [`Exec::Grain`] is chosen.

use crate::{CHECK_EVERY, Config, Host, show};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use core::cell::{Cell, RefCell};
use rhai::{
    AST, Array, Blob, Dynamic, Engine, EvalAltResult, INT, ImmutableString, Map, Position, Scope,
};

/// How compiled code is run. AST stays the default until spike S12-3 shows that Grain
/// gives the same results on the 50 reference scripts and the fuzz corpus and is at
/// least 1.2x faster on the shell's micro-benchmarks (ADR 0012 D9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exec {
    /// Rhai's tree-walking interpreter.
    Ast,
    /// Rhai Grain: the AST lowered to bytecode and run on Grain's VM. What Grain
    /// cannot lower yet goes back to the AST interpreter, so the meaning is the same.
    #[cfg(feature = "grain")]
    Grain,
}

/// Runs `ast` in `scope` the way `exec` says.
pub(crate) fn eval(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    exec: Exec,
) -> Result<Dynamic, Box<EvalAltResult>> {
    match exec {
        Exec::Ast => engine.eval_ast_with_scope::<Dynamic>(scope, ast),
        #[cfg(feature = "grain")]
        Exec::Grain => {
            let program = rhai::grain::Compiler::new().compile(ast);
            rhai::grain::Vm::new(engine).eval_with_scope(scope, &program)
        }
    }
}

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
    register_checked_replace(&mut engine);
    register_checked_pad(&mut engine);
    let capture = Rc::clone(line);
    let max_output = config.max_output;
    engine.on_print(move |text| capture_output(&capture, max_output, text));
    let capture = Rc::clone(line);
    engine.on_debug(move |text, _, _| capture_output(&capture, max_output, text));
    let watch = Rc::clone(line);
    let watch_host = Rc::clone(host);
    let heap_budget = config.heap_budget;
    engine.on_progress(move |operations| {
        // The heap is checked on every operation, since one statement can allocate
        // a lot; the clock and Ctrl-C only every CHECK_EVERY operations.
        let used = watch_host
            .heap_bytes()
            .saturating_sub(watch.heap_start.get());
        let stop = if used > heap_budget {
            Some(Stop::Memory)
        } else if !operations.is_multiple_of(CHECK_EVERY) {
            None
        } else if watch_host.interrupted() {
            Some(Stop::Interrupted)
        } else if watch_host.now_ns() > watch.deadline_ns.get() {
            Some(Stop::TimeLimit)
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

/// Rhai's own `replace` builds the whole new string before any size check, so one
/// call could grow a string far past the limit (and the heap past the shell's
/// quota). These replace the two forms that can grow, checking the size first.
fn register_checked_replace(engine: &mut Engine) {
    let max = engine.max_string_size();
    engine.register_fn(
        "replace",
        move |text: &mut ImmutableString, find: &str, with: &str| {
            replace_within(text, find, with, max)
        },
    );
    engine.register_fn(
        "replace",
        move |text: &mut ImmutableString, find: char, with: &str| {
            replace_within(text, find.encode_utf8(&mut [0; 4]), with, max)
        },
    );
}

fn replace_within(
    text: &mut ImmutableString,
    find: &str,
    with: &str,
    max: usize,
) -> Result<(), Box<EvalAltResult>> {
    if text.is_empty() {
        return Ok(());
    }
    // An empty pattern matches before every character and at the end.
    let count = if find.is_empty() {
        text.chars().count() + 1
    } else {
        text.matches(find).count()
    };
    let size = (text.len() - count * find.len()).saturating_add(count.saturating_mul(with.len()));
    if max > 0 && size > max {
        return Err(EvalAltResult::ErrorDataTooLarge(
            "Length of string after replace".into(),
            Position::NONE,
        )
        .into());
    }
    *text = text.replace(find, with).into();
    Ok(())
}

/// Deepest nesting `deep_size` looks into; anything deeper counts as too large.
const SIZE_DEPTH: usize = 32;

/// Elements and bytes in `value`, nested values included. Bytes count strings,
/// blobs and map keys; Rhai's own limits leave keys out.
fn deep_size(value: &Dynamic, depth: usize) -> (usize, usize) {
    if depth > SIZE_DEPTH {
        return (usize::MAX, usize::MAX);
    }
    if let Some(array) = value.read_lock::<Array>() {
        return array_size(&array, depth);
    }
    if let Some(map) = value.read_lock::<Map>() {
        return map
            .iter()
            .fold((map.len(), 0), |(elements, bytes), (key, item)| {
                let (more, size) = deep_size(item, depth + 1);
                (
                    elements.saturating_add(more),
                    bytes.saturating_add(key.len()).saturating_add(size),
                )
            });
    }
    if let Some(text) = value.read_lock::<ImmutableString>() {
        return (0, text.len());
    }
    if let Some(blob) = value.read_lock::<Blob>() {
        return (blob.len(), 0);
    }
    (0, 0)
}

fn array_size(array: &Array, depth: usize) -> (usize, usize) {
    array
        .iter()
        .fold((array.len(), 0), |(elements, bytes), item| {
            let (more, size) = deep_size(item, depth + 1);
            (elements.saturating_add(more), bytes.saturating_add(size))
        })
}

/// Rhai's own array `pad` checks only the new length before copying its item that
/// many times, so one call could copy a map with long keys, or a nested array, a
/// thousand times over. This one checks everything the copies would hold first.
fn register_checked_pad(engine: &mut Engine) {
    let (max_elements, max_bytes) = (engine.max_array_size(), engine.max_string_size());
    engine.register_fn(
        "pad",
        move |array: &mut Array, len: INT, item: Dynamic| -> Result<(), Box<EvalAltResult>> {
            let Ok(len) = usize::try_from(len) else {
                return Ok(());
            };
            if len <= array.len() {
                return Ok(());
            }
            let added = len - array.len();
            let (elements, bytes) = array_size(array, 0);
            let (item_elements, item_bytes) = deep_size(&item, 1);
            let elements =
                elements.saturating_add(added.saturating_mul(item_elements.saturating_add(1)));
            let bytes = bytes.saturating_add(added.saturating_mul(item_bytes));
            if (max_elements > 0 && elements > max_elements) || (max_bytes > 0 && bytes > max_bytes)
            {
                return Err(EvalAltResult::ErrorDataTooLarge(
                    "Size of array after pad".into(),
                    Position::NONE,
                )
                .into());
            }
            array.resize(len, item);
            Ok(())
        },
    );
}
