//! The Rhai shell, kept free of system calls so it can be tested on the host. The
//! ring-3 `shell` program supplies a [`Host`] and feeds it one line at a time.
//!
//! Like TempleOS's prompt, every line is code: variables and functions persist, and
//! the value of the last expression is echoed. Plain commands (`mem`, `policy rr`,
//! `ask why is the sky blue`) are recognised from their first word and run directly.
//! A line starting with `?`, or plain words that do not run as Rhai, go to the LLM,
//! which answers in words or with a script. A script is checked, then its tier
//! (`tiers`) decides: one whose every function can be undone runs at once and the
//! reply says what happened; one that cannot asks one short question first (see
//! `actions`). `undo` reverts the last run, and `show` or `why` shows its script.
//!
//! Every run is bounded: by time (checked every [`CHECK_EVERY`] operations), by an
//! operation count, by heap use, and by Rhai's own size and depth limits. A run that
//! hits a bound returns an error and leaves the shell running.

#![no_std]

extern crate alloc;

mod actions;
mod commands;
mod engine;
mod log;
pub mod script;
mod show;
pub mod tiers;
mod tune;
mod undo;

use actions::{Pending, Undoable};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use engine::Line;
use kabi::sched::{SchedStats, TrialRequest};
use llmproto::chat::Completion;
use llmproto::script::{Answer, AnswerError, ScriptRequest};
use llmproto::tune::TuneRequest;
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};
use tiers::Tier;

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
    /// Asks the LLM a question; its reply, or why there is none.
    fn ask(&self, prompt: &str) -> Result<String, String>;
    /// Asks the LLM for a script.
    fn script(&self, request: &ScriptRequest) -> Result<Answer, AnswerError>;
    /// Writes one line to the console log.
    fn audit(&self, text: &str);
    /// The scheduler's numbers over `window_ms` (0: the trial's status only).
    fn sched_stats(&self, _window_ms: u64) -> Result<Box<SchedStats>, kabi::Error> {
        Err(kabi::Error::Unavailable)
    }
    /// Starts a kernel-owned policy trial; its id.
    fn sched_trial(&self, _request: &TrialRequest) -> Result<u64, kabi::Error> {
        Err(kabi::Error::Unavailable)
    }
    /// Asks the LLM to tune; its raw completion.
    fn tune(&self, _request: &TuneRequest) -> Result<Completion, String> {
        Err(String::from("tuning is not available here"))
    }
    /// Installs policy `id` with a preset.
    fn set_spec(&self, id: u64, _preset: u64) -> Result<(), kabi::Error> {
        self.set_policy(id)
    }
    fn sleep_ms(&self, _ms: u64) {}
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Longest a line may compute, not counting time spent waiting in `ask`.
    pub line_budget_ms: u64,
    /// Heap a line may add before it is stopped.
    pub heap_budget: u64,
    /// Heap an LLM-written script may add, in its trial run and its real one. Far
    /// smaller than `heap_budget`: one operation can copy a value several times over
    /// (a clone, or text formatted with escapes), the trial runs before anyone has
    /// read the script, and the shell has 16 MiB in all.
    pub script_heap_budget: u64,
    /// Most bytes of printed output kept per line.
    pub max_output: usize,
    /// Backstop if the clock check never fires. Far above what the time budget
    /// allows on fast hardware (KVM runs over 10 million operations a second), so
    /// the time limit is what normally stops a line.
    pub max_operations: u64,
}

pub const DEFAULT_CONFIG: Config = Config {
    line_budget_ms: 2000,
    heap_budget: 8 * 1024 * 1024,
    script_heap_budget: 1024 * 1024,
    max_output: 6 * 1024,
    max_operations: 2_000_000_000,
};

/// How often the time, interrupt and heap checks run, in operations.
pub const CHECK_EVERY: u64 = 1024;
/// Time the llm service gets for one script request; two rounds (an answer and one
/// repair) must fit in the terminal's wait for a line (310 s since the M10 demo).
pub const SCRIPT_ROUND_MS: u64 = 70_000;
/// First words that mark a line as Rhai even when it does not parse.
const RHAI_WORDS: &[&str] = &[
    "let", "const", "fn", "if", "else", "for", "while", "loop", "return", "print", "throw", "try",
    "import",
];

/// The shell's answer to one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub ok: bool,
    pub text: String,
    /// Set when the reply asks the user about script `id`.
    pub confirm: Option<u64>,
}

impl Reply {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
            confirm: None,
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
            confirm: None,
        }
    }

    /// `ok <text>`, `err <text>` or `confirm <id> <text>`, as sent to the terminal.
    pub fn to_wire(&self) -> String {
        match self.confirm {
            Some(id) => format!("confirm {id} {}", self.text),
            None => format!("{} {}", if self.ok { "ok" } else { "err" }, self.text),
        }
    }
}

pub struct Shell {
    engine: Engine,
    scope: Scope<'static>,
    /// Every function defined so far, and no statements.
    functions: AST,
    line: Rc<Line>,
    host: Rc<dyn Host>,
    config: Config,
    pending: Option<Pending>,
    next_id: u64,
    log: log::Log,
    tuner: tune::Tuner,
    /// Each script API function's tier.
    tiers: &'static [(&'static str, Tier)],
    /// The last run's changes, for `undo`.
    undo: Option<Undoable>,
    /// An `undo` waiting for a yes, because something changed it since.
    undo_asking: Option<Undoable>,
    /// The last script as `show` prints it.
    shown: Option<String>,
    /// What the script run by the last line printed.
    last_output: Option<String>,
}

impl Shell {
    pub fn new(host: Rc<dyn Host>, config: Config) -> Self {
        Self::with_tiers(host, config, tiers::TIERS)
    }

    /// A shell whose scripts are tiered by `tiers` instead of [`tiers::TIERS`].
    pub fn with_tiers(
        host: Rc<dyn Host>,
        config: Config,
        tiers: &'static [(&'static str, Tier)],
    ) -> Self {
        let line = Line::new();
        let mut engine = engine::bounded(&line, &host, &config);
        commands::register(&mut engine, &host, &line, config.line_budget_ms);
        Self {
            engine,
            scope: Scope::new(),
            functions: AST::empty(),
            line,
            host,
            config,
            pending: None,
            next_id: 1,
            log: log::Log::default(),
            tuner: tune::Tuner::default(),
            tiers,
            undo: None,
            undo_asking: None,
            shown: None,
            last_output: None,
        }
    }

    /// What the script the last line ran printed, then its error if it failed;
    /// `None` if the line ran no script.
    pub fn last_output(&self) -> Option<&str> {
        self.last_output.as_deref()
    }

    /// Runs one line and returns what to show.
    pub fn eval(&mut self, input: &str) -> Reply {
        let input = input.trim();
        self.last_output = None;
        // `answer <id> <text>` comes from the terminal's confirm prompt. A line such as
        // `answer + 1` is Rhai about a variable, so the id must be a number.
        if let Some(rest) = input.strip_prefix("answer ")
            && rest
                .split(' ')
                .next()
                .is_some_and(|id| id.parse::<u64>().is_ok())
        {
            return self.answer(rest);
        }
        if let Some(reply) = self.answer_bare(input) {
            return reply;
        }
        // Anything but the answer leaves a proposal unrun, and an undo for later.
        self.tuner.discard();
        if let Some(undoable) = self.undo_asking.take() {
            self.undo = Some(undoable);
        }
        if let Some(pending) = self.pending.take() {
            let outcome = "discarded: another line came first";
            self.record(
                pending.id,
                &pending.request,
                &pending.checked.script,
                outcome,
            );
        }
        self.line.begin(self.host.as_ref(), &self.config);
        if input.is_empty() {
            return Reply::ok("");
        }
        if input == "reset" {
            self.scope.clear();
            self.functions = AST::empty();
            return Reply::ok("variables and functions cleared");
        }
        if !self.scope.contains(input) {
            match input {
                "undo" => return self.undo(),
                "show" | "why" => return self.show_last(),
                _ => {}
            }
        }
        if (input == "tune" || input.starts_with("tune ")) && !self.scope.contains("tune") {
            let id = self.next_id;
            self.next_id += 1;
            return self.tuner.command(self.host.as_ref(), &input[4..], id);
        }
        if let Some(request) = input.strip_prefix('?') {
            return self.ask_for_script(request.trim());
        }
        if let Some((name, rest)) = split_command(input, |name| self.scope.contains(name)) {
            if name == "log" {
                return self.show_log(rest.trim());
            }
            return match commands::run(self.host.as_ref(), name, rest) {
                Ok(text) => Reply::ok(text),
                Err(error) => Reply::err(error),
            };
        }
        let ast = match self.engine.compile_with_scope(&self.scope, input) {
            Ok(ast) => ast,
            Err(_) if reads_like_words(input) => return self.ask_for_script(input),
            Err(error) => return Reply::err(format!("syntax error: {error}")),
        };
        let program = self.functions.merge(&ast);
        let result = self
            .engine
            .eval_ast_with_scope::<Dynamic>(&mut self.scope, &program);
        self.compact_scope();
        let mut text = self.line.take_output();
        match result {
            Ok(value) => {
                self.functions = self.functions.merge(&ast.clone_functions_only());
                if !value.is_unit() {
                    text.push_str(&show::value(&value));
                }
                Reply::ok(text)
            }
            // A word Rhai took for a variable, in a line of plain words: `hello`.
            // A one-letter or code-like name (`x`, `n2`, `max_len`) stays an error.
            Err(error)
                if text.is_empty()
                    && reads_like_words(input)
                    && matches!(
                        error.unwrap_inner(),
                        EvalAltResult::ErrorVariableNotFound(name, _) if is_word(name)
                    ) =>
            {
                self.ask_for_script(input)
            }
            Err(error) => {
                text.push_str(&self.describe(&error));
                Reply::err(text)
            }
        }
    }

    fn describe(&self, error: &EvalAltResult) -> String {
        self.line.describe(error, &self.config)
    }

    fn show_log(&self, which: &str) -> Reply {
        if which.is_empty() {
            return Reply::ok(self.log.list());
        }
        match which
            .trim_start_matches('#')
            .parse::<u64>()
            .ok()
            .and_then(|id| self.log.script(id))
        {
            Some(text) => Reply::ok(text),
            None => Reply::err(format!("no logged script {which}")),
        }
    }

    /// Asks the LLM for a script for `request`, checks it, and proposes it. A refused
    /// answer goes back to the model once, with the reason.
    fn ask_for_script(&mut self, request: &str) -> Reply {
        if request.is_empty() {
            return Reply::err("usage: ? <what you want done>");
        }
        let id = self.next_id;
        self.next_id += 1;
        self.host.audit(&format!(
            "script {id} requested: {}",
            show::truncate(request, 60)
        ));
        let mut repair: Option<(String, String)> = None;
        let mut outcome = String::new();
        for round in 0..2 {
            let ask = ScriptRequest {
                request: request.to_string(),
                repair: repair.take(),
                timeout_ms: SCRIPT_ROUND_MS,
            };
            let failure = match self.host.script(&ask) {
                // No script: the model answered in words.
                Err(AnswerError::NoScript(reply)) if !reply.is_empty() => {
                    return self.say(id, request, reply);
                }
                Ok(answer) => match script::check(&answer, &self.host, &self.config) {
                    Ok(checked) => match self.tier_of(request, &checked) {
                        (Tier::Never, at) => Some((
                            answer.script,
                            format!("{} may not be used in scripts here", at.join(", ")),
                        )),
                        tier => return self.propose(id, request, checked, tier),
                    },
                    Err(script::Refusal::Script(why)) => Some((answer.script, why)),
                    Err(refusal @ script::Refusal::Interrupted) => {
                        outcome = refusal.to_string();
                        None
                    }
                },
                Err(error) if error.repairable() => Some((
                    String::from("(the answer was refused before it was read)"),
                    error.describe(),
                )),
                Err(error) => {
                    outcome = error.describe();
                    None
                }
            };
            match failure {
                Some((script, why)) if round == 0 => repair = Some((script, why)),
                Some((_, why)) => outcome = format!("gave up after one repair: {why}"),
                None => break,
            }
        }
        self.log
            .record(id, request, "", &format!("refused: {outcome}"));
        self.host.audit(&format!(
            "script {id} refused: {}",
            show::truncate(&outcome, 120)
        ));
        Reply::err(outcome)
    }

    fn say(&mut self, id: u64, request: &str, reply: String) -> Reply {
        self.log.record(id, request, "", "answered in words");
        self.host.audit(&format!("script {id} answered in words"));
        Reply::ok(reply)
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

/// A line that does not run as Rhai is sent to the LLM if it reads like words: mostly
/// letters, no code punctuation, and not starting with a Rhai keyword.
pub fn reads_like_words(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() || RHAI_WORDS.contains(&words[0]) {
        return false;
    }
    if line.chars().any(|c| "(){}[];=\"`#$".contains(c)) {
        return false;
    }
    let letters = line.chars().filter(|c| c.is_alphabetic()).count();
    let others = line
        .chars()
        .filter(|c| !c.is_alphabetic() && !c.is_whitespace())
        .count();
    letters >= 4 * others
}

/// A name that reads as a word, not as code: two or more letters and nothing else.
fn is_word(name: &str) -> bool {
    name.chars().count() > 1 && name.chars().all(char::is_alphabetic)
}

/// A Rhai error from a message.
fn runtime_error(message: impl Into<String>) -> alloc::boxed::Box<EvalAltResult> {
    let message: String = message.into();
    message.into()
}

#[cfg(test)]
mod tests;
