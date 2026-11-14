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
//!
//! An entry with a brace, bracket or parenthesis still open is kept, and the reply
//! is `more`: the next line continues it (the terminal shows `..> `).

#![no_std]

extern crate alloc;

mod actions;
pub mod actlog;
mod agentcore;
pub mod analyse;
pub mod apart;
pub mod appbuild;
mod appearance;
mod appinstall;
pub mod appjob;
pub mod appline;
pub mod apps;
mod brackets;
pub mod ccbuild;
mod centry;
mod cmode;
mod commands;
pub mod data;
mod deskask;
pub mod door;
mod effects;
mod engine;
pub mod grants;
mod guard;
mod instructions;
mod models;
mod netask;
pub mod netgrant;
pub use models::MODELS_APP;
pub use netask::Connect as NetConnect;
mod packages;
mod panel;
mod pkg;
mod profiles;
mod rules;
pub mod script;
mod settings;
pub use settings::{MAX_ANSWER as SETTINGS_MAX_ANSWER, SETTINGS_APP};
mod show;
mod sitewords;
pub mod standing;
pub mod state;
pub mod targets;
pub mod tiers;
mod tune;
pub mod ui;
mod undo;
mod undoing;

use actions::Pending;
pub use agentcore::Outcome;
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use centry::CallValue;
/// Repairs a Kai C program at `c>` gets after its first answer (P4 task 14).
pub use cmode::C_REPAIRS;
pub use effects::EffectDone;
pub use engine::Exec;
use engine::Line;
pub use guard::placeholder_target;
use kabi::sched::{SchedStats, TrialRequest};
use llmproto::act::{ActError, ActRequest, Call};
use llmproto::chat::Completion;
use llmproto::script::{Answer, AnswerError, Lang, ScriptRequest};
use llmproto::tune::TuneRequest;
pub use panel::PanelResult;
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};
use tiers::Tier;

pub use commands::{COMMANDS, split_command};

/// What lets an asked call run, shown to the call's owner (the kernel for a
/// process, the supervisor for a service), which checks it: the shell only
/// passes it on (P1 task 14, ADR 0001 D6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Proof {
    /// Nothing: an owner that checks refuses an asked call.
    #[default]
    None,
    /// The `ONCE` the trusted path minted for menu `id`'s Allow.
    Once { badge: u64, id: u64 },
    /// A standing rule's node in the shell's table, which the owner finds.
    Rule,
    /// The undo of log entry `id`'s start of the service, which the supervisor
    /// remembers.
    Undo { id: u64 },
}

/// A read the `fs` server answers (P2 task 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsRead<'a> {
    List(&'a str),
    Read(&'a str),
    /// The whole file or an error, for the editor's door: no cut at a few
    /// messages (`Read` hands a script a prefix), and it must be text.
    Whole(&'a str),
    History(&'a str),
    Changes,
}

/// A change the `fs` server makes as one transaction, which `undo` restores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsChange<'a> {
    Create { path: &'a str, text: &'a str },
    Write { path: &'a str, text: &'a str },
    Move { from: &'a str, to: &'a str },
    Trash(&'a str),
    Restore(&'a str),
    EmptyTrash,
}

/// Why the `fs` server did not restore a transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsRefused {
    /// A later change touched the same files, as this says; restoring would undo
    /// it too, so the user is asked first.
    Later(String),
    Failed(String),
}

/// What a run of log entry `entry` may show the owners of its calls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Consent {
    pub entry: u64,
    pub proof: Proof,
}

/// Why a network row cannot run on a host without the network.
pub const NO_NET: &str = "the network rows need the OS network service here";
/// What a host with no `kai-data` disk answers a file row: the guest's shell
/// and the gates' stand-ins say the same, so their answers compare equal.
pub const NO_FILES: &str = "there are no files here (no kai-data disk)";

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
    /// Starts asking the LLM for `request` off the line, for an app's round
    /// (`appjob`); its answer comes from [`Host::script_take`]. A host that
    /// cannot says so, and the line asks with [`Host::script`] instead.
    fn script_start(&self, _request: &ScriptRequest) -> appjob::Started {
        appjob::Started::Inline
    }
    /// The answer to a [`Host::script_start`] that has come, with its ticket.
    fn script_take(&self) -> Option<(u64, Result<Answer, AnswerError>)> {
        None
    }
    /// Stops the [`Host::script_start`] with `ticket`, if it is still asking.
    fn script_stop(&self, _ticket: u64) {}
    /// Which script prompt the model should keep warm: C's at `c>`, Rhai's at
    /// `kai>`. A hint, so nothing comes back.
    fn model_warm(&self, _lang: Lang) {}
    /// Asks the LLM for one tagged call (ADR 0002 D4). Without one, the shell asks
    /// for a script instead.
    fn act(&self, _request: &ActRequest) -> Result<Call, ActError> {
        Err(ActError::Unavailable(String::from("no tagged calls here")))
    }
    /// Keeps a finished model turn in llm's conversation memory (P1 task 10): the
    /// user's line, the call as `act` returned it, and the result line.
    fn remember(&self, _line: &str, _call: &str, _result: &str) {}
    /// Forgets the conversation (`new`).
    fn forget(&self) {}
    /// Where the user is, for the State line (ADR 0010 §5); nothing by default.
    fn state(&self) -> state::State {
        state::State::default()
    }
    /// Whether `digest` is the one the trusted path holds for the `ONCE` badge
    /// `badge` (ONCE_CHECK); a match uses it up. A host without the trusted path
    /// has no Allow to check, so an asked call does not run on its menu.
    fn once_check(&self, _badge: u64, _digest: &[u8; 32]) -> Result<(), String> {
        Err(String::from("there is no trusted path here"))
    }
    /// Stops every running process named `name`; how many stopped. `proof` is
    /// what the kernel checks before it stops anything (P1 task 14).
    fn stop_process(&self, _name: &str, _proof: Proof) -> Result<u64, String> {
        Err(String::from("processes cannot be stopped here"))
    }
    /// Stops the supervised service `name`, which stays stopped until started.
    /// `proof` is what the supervisor checks.
    fn stop_service(&self, _name: &str, _proof: Proof) -> Result<(), String> {
        Err(String::from("services cannot be stopped here"))
    }
    /// Starts the stopped service `name` for log entry `entry`, which the
    /// supervisor keeps so only that entry's undo may stop it again unasked.
    fn start_service(&self, _name: &str, _entry: u64) -> Result<(), String> {
        Err(String::from("services cannot be started here"))
    }
    /// Stops and starts the supervised service `name` in one request: a ONCE is
    /// used up by its first check, so two requests could not both pass.
    fn restart_service(&self, _name: &str, _proof: Proof) -> Result<(), String> {
        Err(String::from("services cannot be restarted here"))
    }
    /// The installed apps' rows and the effects their calls queued (P4);
    /// `None` where there are no apps.
    fn apps(&self) -> Option<&core::cell::RefCell<apps::Apps>> {
        None
    }
    /// A `kai-call 1`, `kai-undo 1` or `kai-actions 1` to app `app`, through the
    /// supervisor's `app-call`: the app's reply, or why not.
    fn app_call(&self, _app: &str, _request: &str) -> Result<String, String> {
        Err(String::from("there are no apps here"))
    }
    /// One of `updated`'s model requests (`models`, `models import`, `models
    /// progress`, `models use <64 hex>`; P4 task 10): its answer, or why there
    /// is none.
    fn models(&self, _request: &str) -> Result<String, String> {
        Err(String::from("there is no model channel here"))
    }
    /// The user's appearance as `kai-appearance 1` text (P4 task 8).
    fn appearance_read(&self) -> Option<String> {
        None
    }
    /// Saves the appearance and has `desktop` draw with it.
    fn appearance_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("there is no appearance to set here"))
    }
    /// One of the agent's window rows, asked of `desktop` (P3 task 10): the
    /// answer after its `ok`, or why not.
    fn ui(&self, _call: &ui::UiCall) -> Result<String, String> {
        ui::no_desktop()
    }
    /// One `kai-cc 1` message to the `cc` service, and its reply (ADR 0012;
    /// the `c>` prompt).
    fn cc(&self, _request: &[u8]) -> Result<Vec<u8>, String> {
        Err(String::from("there is no c> service here"))
    }
    /// One request to `pkg` through the shell's route to it: the text after
    /// its `ok`, or why not (P4 task 13).
    fn pkg(&self, _request: &str) -> Result<String, String> {
        Err(String::from("there is no pkg service here"))
    }
    /// `cc build`'s work outside the shell's logic: component `name`'s
    /// sources, read from `dir/name/` ([`ccbuild::DIR`], or [`ccbuild::APPS_DIR`]
    /// for `app_build`), built by `cc` at `version`, and the artifact written
    /// beside them.
    fn cc_build(
        &self,
        _dir: &str,
        _name: &str,
        _version: &str,
    ) -> Result<ccbuild::CcBuilt, String> {
        Err(String::from("cc build needs the files and cc here"))
    }
    /// `cc`'s `trial` of the app the last [`Host::cc_build`] built, on stub
    /// imports: its one-line report, or why it failed. A failure that is not
    /// the program's fault starts [`appbuild::INTERNAL`].
    fn cc_trial(&self, _name: &str) -> Result<String, String> {
        Err(format!(
            "{}there is no cc to try the app on here",
            appbuild::INTERNAL
        ))
    }
    /// The artifact app `name`'s build wrote, its hash and imports: `None`
    /// unless it is the last one the host wrote, so an install card never
    /// shows a list, or installs bytes, it does not have.
    fn cc_artifact(&self, _name: &str) -> Option<ccbuild::Artifact> {
        None
    }
    /// Writes `text` as the whole of `/home/user/apps/<name>/main.c`, making the
    /// folders if they are new (`app_build`). The source is kept whatever comes
    /// of its build, so it is no undo of its own.
    fn app_source(&self, _name: &str, _text: &str) -> Result<(), String> {
        Err(String::from("there are no files here"))
    }
    /// `/home/user/apps/<name>/main.c` as it is now: `None` if there is none.
    fn app_source_read(&self, _name: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    /// Hands the artifact at `path` to `pkg`'s `local` as a read-only File:
    /// the text after its `ok`. `pkg` installs only the bytes `cc` built, and
    /// with `sha256`, only if that is their hash.
    fn pkg_local(&self, _name: &str, _path: &str, _sha256: Option<&str>) -> Result<String, String> {
        Err(String::from("there is no pkg service here"))
    }
    /// `net_fetch` (P5 task 11): the text of `path` on `host`, which consent
    /// grounded. None here until kainet's HTTP client is wired in.
    fn net_fetch(&self, _host: &str, _path: &str) -> Result<String, String> {
        Err(String::from(NO_NET))
    }
    /// `net_resolve`: the addresses `host` resolves to.
    fn net_resolve(&self, _host: &str) -> Result<Vec<String>, String> {
        Err(String::from(NO_NET))
    }
    /// `query` (P5 task 11): the model's answer to one typed question
    /// about read data, asked apart from every other request. `llm` checks it
    /// against `request.kind`, and the script binding checks it again.
    fn query(&self, _request: &llmproto::query::QueryRequest) -> Result<String, String> {
        Err(String::from("no model answers questions here"))
    }
    /// `net_ping`: how many ms `host` takes to answer, or -1 for none.
    fn net_ping(&self, _host: &str) -> Result<i64, String> {
        Err(String::from(NO_NET))
    }
    /// A read of the user's files: its text, or why there is none.
    fn fs_read(&self, _read: FsRead) -> Result<String, String> {
        Err(String::from(NO_FILES))
    }
    /// A change to the user's files, made as one transaction: its number, for
    /// `undo`.
    fn fs_change(&self, _change: FsChange) -> Result<u64, String> {
        Err(String::from(NO_FILES))
    }
    /// The action log as `/system/log` holds it (`actlog 1`), read once at start;
    /// `None` when there is none (P2 task 12).
    fn log_read(&self) -> Option<String> {
        None
    }
    /// Writes the whole action log to `/system/log`, after any line that changed it.
    fn log_write(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    /// Restores what transaction `txn` replaced, as one new transaction: its number,
    /// so `undo` again can redo. Compare-and-set: refused when a later change
    /// touched the same files, unless `anyway`.
    fn fs_undo(&self, _txn: u64, _anyway: bool) -> Result<u64, FsRefused> {
        Err(FsRefused::Failed(String::from(NO_FILES)))
    }
    /// The services the supervisor runs, so a stop names a service or a process
    /// by what it is. Without them, a stop runs the row the model picked.
    fn services(&self) -> Result<Vec<targets::Service>, String> {
        Err(String::from("services cannot be listed here"))
    }
    /// Revokes a standing rule's grant node, through the `Grant` handle the
    /// trusted path gave for it.
    fn revoke_rule(&self, _handle: u32) -> Result<(), String> {
        Ok(())
    }
    /// The standing rules as `/system/state/rules` holds them (`kai-grants 1`
    /// rule lines), read once at start; `None` when there are none (P2 task 12).
    fn rules_read(&self) -> Option<String> {
        None
    }
    /// Writes the whole rules file, after a rule is added or removed.
    fn rules_write(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    /// The keep-running list as `/system/state/keep` holds it.
    fn keep_read(&self) -> Option<String> {
        None
    }
    fn keep_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("there is no /system/state here"))
    }
    /// `keep <app> on|off` to the supervisor: app `app` naps or is kept running.
    fn keep_running(&self, _app: &str, _on: bool) -> Result<(), String> {
        Err(String::from("apps cannot be kept running here"))
    }
    /// The restart window as `/system/state/restart/window` holds it
    /// (`kai-updates 1`), which `updated` follows.
    fn restart_read(&self) -> Option<String> {
        None
    }
    fn restart_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("there is no /system/state here"))
    }
    /// The rows taken back from apps' effects, as `/system/state/withheld`
    /// holds them (`kai-withheld 1`, P4 task 7).
    fn withheld_read(&self) -> Option<String> {
        None
    }
    /// Writes the whole withheld file, before a change takes effect.
    fn withheld_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("grants cannot be revoked here"))
    }
    /// The apps' network grants as `/system/state/netgrants` holds them
    /// (`kai-grants 1` host lines, P5 task 13).
    fn net_grants_read(&self) -> Option<String> {
        None
    }
    /// Writes the whole network grants file, before a change is told.
    fn net_grants_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("network grants cannot be kept here"))
    }
    /// The standing instructions as `/system/state/standing` holds them
    /// (`kai-standing 1`), read once at start (P3 task 11).
    fn standing_read(&self) -> Option<String> {
        None
    }
    /// Writes the whole standing-instruction store, after one changes.
    fn standing_write(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    /// Gives each process its class and quota (P3 task 12), through the
    /// supervisor's PROC_CLASS_SET and PROC_QUOTA_SET; every one's settings from
    /// before, so `undo` can put them back.
    fn profile_apply(
        &self,
        _settings: &[kaiprofile::Setting],
    ) -> Result<Vec<kaiprofile::Setting>, String> {
        Err(String::from(
            "profiles are not wired to the supervisor here yet",
        ))
    }
    /// The app with focus on the desktop, which a profile's `focused` line names.
    fn focused_app(&self) -> Option<String> {
        None
    }
    /// Where standing instructions' events come from and their actions run;
    /// `None` until the desktop and comphost are wired (P3 tasks 6 and 11).
    fn triggers(&self) -> Option<&dyn standing::Triggers> {
        None
    }
    /// Runs `request` ([`apart::request`]) in a `scriptrun` child of its own (P1
    /// task 14): `Ok` with the child's answer, or `Err` with why it ended without
    /// one (stopped for time or Ctrl-C, or it faulted). `None` when there is no
    /// child here, so the shell runs the script itself.
    fn run_apart(&self, _request: &str) -> Option<Result<String, String>> {
        None
    }
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
    /// How typed lines and scripts are run.
    pub exec: Exec,
}

pub const DEFAULT_CONFIG: Config = Config {
    line_budget_ms: 2000,
    heap_budget: 8 * 1024 * 1024,
    script_heap_budget: 1024 * 1024,
    max_output: 6 * 1024,
    max_operations: 2_000_000_000,
    exec: Exec::Ast,
};

/// Longest entry the shell gathers over continuation lines (ADR 0012, "Modes").
pub const MAX_ENTRY: usize = 4 * 1024;
/// What the terminal sends when Ctrl-C clears a continuation line: the entry so far
/// is dropped. A typed line cannot hold a control character.
pub const CANCEL_ENTRY: &str = "\u{3}";

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
    /// Set when a brace, bracket or parenthesis is still open: the next line
    /// continues this entry, and the terminal shows `..> `.
    pub more: bool,
}

impl Reply {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
            confirm: None,
            more: false,
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
            confirm: None,
            more: false,
        }
    }

    fn more() -> Self {
        Self {
            more: true,
            ..Self::ok("")
        }
    }

    /// The Allow / Deny menu for call `id`, drawn by the terminal: `text` is
    /// `termcore::menu::Menu::encode`'s.
    fn menu(id: u64, text: String) -> Self {
        Self {
            ok: true,
            text,
            confirm: Some(id),
            more: false,
        }
    }

    /// `ok <text>`, `err <text>`, `confirm <id> <text>`, `menu <id> ...` or `more`,
    /// as sent to the terminal.
    pub fn to_wire(&self) -> String {
        match self.confirm {
            _ if self.more => String::from("more"),
            Some(_) if self.text.starts_with("menu ") => self.text.clone(),
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
    /// The action log: every request, what it changed, and its undo.
    log: actlog::ActLog,
    tuner: tune::Tuner,
    /// Each script API function's tier.
    tiers: &'static [(&'static str, Tier)],
    /// The entry an `undo` waits on a yes for, because something changed it since.
    undo_asking: Option<u64>,
    /// Standing rules the menu's "Always allow" wrote.
    rules: rules::Rules,
    /// Standing instructions (P3 task 11), and the ones `rules_remove` ended,
    /// which `undo` can put back until the shell restarts.
    standing: standing::Standing,
    instructions_removed: Vec<standing::Rule>,
    /// Resource profiles (P3 task 12), in RAM until `/system/settings`.
    profiles: profiles::Profiles,
    /// The apps kept running, never napped (`/system/state/keep`).
    keep: Vec<String>,
    /// Saved kept apps the supervisor could not reach yet at boot (an app not
    /// serving yet), and when to try them again and when to give up.
    keep_waiting: settings::KeepWaiting,
    /// Each standing instruction's last profile switch, which its "until" puts back.
    fired_profiles: Vec<(u64, u64)>,
    /// What `app_build` built last and has not installed ("Kai builds apps").
    app_built: Option<appbuild::Built>,
    /// Each app's `main.c` this session wrote, by its SHA-256: overwritten
    /// again only while it is still that.
    app_written: Vec<(String, String)>,
    /// The `app_build` going on between lines, if one is.
    app_job: Option<appjob::Job>,
    /// What builds in the background came to, not yet shown.
    notices: Vec<String>,
    /// The app whose install card waits to open by itself, once a build in
    /// the background built it ([`Shell::take_card`]).
    card: Option<String>,
    /// Proposals from desktop lines (the panel, the Terminal window) waiting
    /// for the trusted card (`deskask`).
    desk_asks: Vec<deskask::Waiting>,
    /// The question on the card that came from a desktop line.
    card_for_desk: Option<u64>,
    /// What that card's answer came to, for the panel.
    card_result: Option<(bool, String)>,
    /// Apps' network asks waiting for the terminal's card (P5 task 13).
    net_cards: Vec<netask::NetAsking>,
    /// Network grants revoked, kept for their undo.
    net_revoked: Vec<kaigrants::Grant>,
    /// The highest network grant id this run gave or saw: new grants are
    /// numbered above it, so an id is never reused while the shell runs.
    net_high: u32,
    /// Each network card the person answered Deny: refused with no card for
    /// `netask::DENY_HOLDS_NS`.
    net_denied: Vec<netask::Denied>,
    /// Installed software: the image's repository and its generations.
    pkgs: pkg::Store,
    /// Whether the `sudo` note was said already.
    said_no_sudo: bool,
    /// The words that name a site for the agent's network rows: the curated host
    /// table, and each installed app's word for its own hosts (P5 task 11).
    hosts: nethosts::Table,
    /// The last script as `show` prints it.
    shown: Option<String>,
    /// What the script run by the last line printed.
    last_output: Option<String>,
    /// What the last finished script showed, and the shell's line after it: a
    /// model turn whose reply printed Content is remembered without its text
    /// (#831).
    showed: agentcore::Showed,
    /// An entry with a brace, bracket or parenthesis still open, waiting for its
    /// next line.
    partial: Option<String>,
    /// A request the model asked to clarify: the next line is joined to it.
    clarifying: Option<String>,
    /// The terminals of apps that run shell lines, by app (`appline`): each has an
    /// entry, question and proposal of its own.
    fronts: alloc::collections::BTreeMap<String, panel::Terminal>,
    /// Since the last panel line: whether the line went to the model, whether
    /// it answered in words, and code's result line for what ran.
    asked_model: bool,
    worded: bool,
    done_line: Option<String>,
    /// The `pack_id` of the pack the model is given, logged with each model turn
    /// (ADR 0010 §7): the same one llm reports for its `act` slot.
    pack_id: u64,
    /// The log text last written to `/system/log`, so an unchanged log is not
    /// written again.
    saved_log: Option<String>,
    /// The ONCE of the menu answer now running, kept for the call's owner to
    /// check: the shell does not spend it (P1 task 14).
    owner_once: Option<u64>,
    /// The source of the data the call now running was decided after (a
    /// view's second round), for consent (ADR 0002 D6).
    steered: Option<&'static str>,
    /// The `c>` entry now running, and its calls' values (P4 task 12).
    c_entry: Option<centry::CEntry>,
    last_value: Option<rhai::Dynamic>,
    value_rets: Vec<(u64, agentabi::RetType)>,
    values: Vec<(u64, centry::CallValue)>,
    /// The `c>` prompt's state.
    c: cmode::CMode,
}

impl Shell {
    /// The State the next request carries, `#{...}`, or `None` when the OS knows
    /// nothing to show (ADR 0010 §5).
    pub fn state_line(&self) -> Option<String> {
        self.host.state().snapshot()
    }
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
            log: actlog::ActLog::default(),
            tuner: tune::Tuner::default(),
            tiers,
            undo_asking: None,
            rules: rules::Rules::default(),
            standing: standing::Standing::default(),
            instructions_removed: Vec::new(),
            profiles: profiles::Profiles::default(),
            keep: Vec::new(),
            keep_waiting: settings::KeepWaiting::default(),
            fired_profiles: Vec::new(),
            app_built: None,
            app_written: Vec::new(),
            app_job: None,
            notices: Vec::new(),
            card: None,
            desk_asks: Vec::new(),
            card_for_desk: None,
            card_result: None,
            net_cards: Vec::new(),
            net_revoked: Vec::new(),
            net_high: 0,
            net_denied: Vec::new(),
            pkgs: pkg::Store::default(),
            said_no_sudo: false,
            hosts: nethosts::Table::curated(),
            shown: None,
            last_output: None,
            showed: agentcore::Showed::default(),
            partial: None,
            clarifying: None,
            fronts: alloc::collections::BTreeMap::new(),
            asked_model: false,
            worded: false,
            done_line: None,
            pack_id: llmproto::pack::act_pack_id(),
            saved_log: None,
            owner_once: None,
            steered: None,
            c_entry: None,
            last_value: None,
            value_rets: Vec::new(),
            values: Vec::new(),
            c: cmode::CMode::default(),
        }
        .with_saved_log()
        .with_saved_rules()
        .with_saved_standing()
        .with_saved_keep()
        .with_saved_withheld()
    }

    /// The rows withheld from apps' effects, checked at boot.
    fn with_saved_withheld(mut self) -> Self {
        grants::check_saved(self.host.as_ref());
        self.net_high = netask::at_boot(self.host.as_ref());
        self
    }

    /// The log `/system/log` holds, so `undo` works across a reboot; a text that
    /// is not `actlog 1` is set aside, and the log starts again.
    /// The rules `/system/state/rules` holds; a file refused whole is set aside,
    /// and the shell starts with none.
    fn with_saved_rules(mut self) -> Self {
        let Some(text) = self.host.rules_read() else {
            return self;
        };
        match rules::Rules::restore(&text) {
            Ok(rules) => self.rules = rules,
            Err(why) => self.host.audit(&format!(
                "the rules on disk were refused ({why}); starting with none"
            )),
        }
        self
    }

    /// Writes the rules to `/system/state/rules`; a failure is audited, and the
    /// rules hold in memory.
    pub(crate) fn save_rules(&self) {
        let written = self
            .rules
            .text()
            .and_then(|text| self.host.rules_write(&text));
        if let Err(why) = written {
            self.host
                .audit(&format!("the rules were not written: {why}"));
        }
    }

    fn with_saved_log(mut self) -> Self {
        let Some(text) = self.host.log_read() else {
            // Nothing on disk is an empty log: it needs no write until it changes.
            self.saved_log = Some(self.log.text());
            return self;
        };
        match actlog::parse(&text) {
            Ok(records) => {
                self.log = actlog::ActLog::from_records(records);
                self.next_id = self.log.last_id() + 1;
                self.saved_log = Some(text);
            }
            Err(error) => self.host.audit(&format!(
                "the log on disk is not actlog 1 (line {}: {}); starting a new one",
                error.line, error.why
            )),
        }
        self
    }

    /// Writes the log to `/system/log` if the line changed it.
    fn save_log(&mut self) {
        let text = self.log.text();
        if self.saved_log.as_ref() == Some(&text) {
            return;
        }
        match self.host.log_write(&text) {
            Ok(()) => self.saved_log = Some(text),
            Err(why) => self.host.audit(&format!("the log was not written: {why}")),
        }
    }

    /// What the script the last line ran printed, then its error if it failed;
    /// `None` if the line ran no script.
    pub fn last_output(&self) -> Option<&str> {
        self.last_output.as_deref()
    }

    /// Runs one line and returns what to show; the log goes to disk after it.
    pub fn eval(&mut self, input: &str) -> Reply {
        if let Some(reply) = self.status_line(input) {
            return reply;
        }
        let reply = self.eval_line(input);
        self.save_log();
        reply
    }

    /// [`Shell::eval`] for the terminal's own line: a build's notice goes
    /// ahead of a plain reply, and a question keeps it for the next. The
    /// panel's and apps' lines go through `eval`, so their result stays
    /// theirs (C on #621).
    pub fn eval_console(&mut self, input: &str) -> Reply {
        // The terminal asks for the card the shell said waits: never a typed
        // line, and never the panel's or an app's.
        if input == termcore::menu::TAKE_CARD {
            let reply = self.take_card();
            self.save_log();
            return reply;
        }
        if let Some(reply) = self.status_line(input) {
            return reply;
        }
        let answering = input.strip_prefix("answer ").and_then(|rest| {
            let mut words = rest.split(' ');
            let id = words.next()?.parse::<u64>().ok()?;
            Some((id, words.next().unwrap_or("")))
        });
        if answering.is_some_and(|(id, _)| Some(id) == self.card_for_desk) {
            self.done_line = None;
        }
        let mut reply = self.eval(input);
        self.desk_answered(answering, &reply);
        if reply.confirm.is_none() && !reply.more && !self.notices.is_empty() {
            let mut text = self.take_notices().join("\n");
            if !reply.text.is_empty() {
                text.push('\n');
                text.push_str(&reply.text);
            }
            reply.text = text;
        }
        reply
    }

    /// A line that only reports state: answered before any other line's
    /// handling, so it never discards a pending card, a proposal or an undo
    /// that waits, never takes queued notices, and writes no log. `net-grant`'s
    /// case polls `net denied` while a card waits (#818's batch failures).
    fn status_line(&mut self, input: &str) -> Option<Reply> {
        if self.partial.is_some() || self.scope.contains("net") {
            return None;
        }
        match input.trim() {
            "net denied" => Some(self.net_denied_line()),
            _ => None,
        }
    }

    fn eval_line(&mut self, input: &str) -> Reply {
        // An open Rhai entry takes the next line first, `:c` included.
        if let Some(entry) = self.partial.take() {
            return self.continue_entry(entry, input);
        }
        if let Some(reply) = self.c_line(input) {
            return reply;
        }
        let input = input.trim();
        self.last_output = None;
        if let Some(reply) = self.app_job_line(input) {
            return reply;
        }
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
        self.undo_asking = None;
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
        // The answer to a clarifying question joins the request it clarifies; a typed
        // command or Rhai line instead leaves it.
        if let Some(request) = self.clarifying.take()
            && reads_like_words(input)
            && split_command(input, |name| self.scope.contains(name)).is_none()
        {
            return self.ask_for_script(&format!("{request} {input}"));
        }
        if input == "reset" {
            self.scope.clear();
            self.functions = AST::empty();
            return Reply::ok("variables and functions cleared");
        }
        if !self.scope.contains(input) {
            match input {
                "undo" => return self.undo_command(""),
                "tune undo" => return self.tune_undo(),
                "new" => return self.forget(),
                "rules" => return Reply::ok(self.list_rules()),
                "profile" => return self.list_profiles(),
                "show" | "why" => return self.show_last(),
                _ => {}
            }
        }
        // Installing Kai's last app, or showing its code, comes before a
        // package line of the same words.
        if !self.scope.contains("install")
            && !self.scope.contains("show")
            && let Some(reply) = self.app_install_line(input)
        {
            return reply;
        }
        // Package lines come before Rhai, unless their first word is a variable.
        if let Some(word) = pkg::line::first_word(input)
            && !self.scope.contains(word)
            && let Some(parsed) = pkg::line::parse(input)
        {
            return self.pkg_line(input, parsed);
        }
        // So do `models` lines: with no model yet, a person imports the first.
        if !self.scope.contains("models")
            && let Some(parsed) = models::parse_line(input)
        {
            return self.models_line(input, parsed);
        }
        if let Some(which) = input.strip_prefix("rules_remove ")
            && !self.scope.contains("rules_remove")
        {
            return match which.trim().starts_with('S') {
                true => self.remove_instruction(input, which.trim()),
                false => self.remove_rule(which),
            };
        }
        if let Some(text) = input.strip_prefix("rules_add ")
            && !self.scope.contains("rules_add")
        {
            return self.add_instruction(input, text);
        }
        if let Some(name) = input.strip_prefix("profile ")
            && !self.scope.contains("profile")
        {
            return self.profile_command(input, name);
        }
        if let Some(which) = input.strip_prefix("undo ")
            && !self.scope.contains("undo")
            && which.trim().trim_start_matches('#').parse::<u64>().is_ok()
        {
            return self.undo_command(which);
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
        if brackets::open(input) {
            return self.gather(String::from(input));
        }
        self.run_code(input)
    }

    /// The next line of an entry left open: it is added, and the whole entry runs
    /// once everything it opened is closed.
    fn continue_entry(&mut self, mut entry: String, input: &str) -> Reply {
        self.last_output = None;
        if input == CANCEL_ENTRY {
            return Reply::ok("");
        }
        entry.push('\n');
        entry.push_str(input.trim_end());
        if entry.len() > MAX_ENTRY {
            return Reply::err(format!(
                "dropped: an entry may be at most {} KiB",
                MAX_ENTRY / 1024
            ));
        }
        if brackets::open(&entry) {
            return self.gather(entry);
        }
        self.line.begin(self.host.as_ref(), &self.config);
        self.run_code(&entry)
    }

    /// Keeps `entry` for the next line and asks the terminal for it.
    fn gather(&mut self, entry: String) -> Reply {
        self.partial = Some(entry);
        Reply::more()
    }

    /// Compiles and runs typed Rhai; plain words that do not run go to the LLM.
    fn run_code(&mut self, input: &str) -> Reply {
        let ast = match self.engine.compile_with_scope(&self.scope, input) {
            Ok(ast) => ast,
            Err(_) if reads_like_words(input) => return self.ask_for_script(input),
            Err(error) => return Reply::err(format!("syntax error: {error}")),
        };
        let program = self.functions.merge(&ast);
        let result = engine::eval(&self.engine, &mut self.scope, &program, self.config.exec);
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
        match which {
            "" => return Reply::ok(self.log.list()),
            "actlog" => return Reply::ok(self.log.text()),
            _ => {}
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
        self.asked_model = true;
        let id = self.next_id;
        self.next_id += 1;
        self.host.audit(&format!(
            "script {id} requested: {}",
            show::truncate(request, 60)
        ));
        if let Some(reply) = self.try_act(id, request) {
            return reply;
        }
        let mut repair: Option<(String, String)> = None;
        let mut outcome = String::new();
        for round in 0..2 {
            let ask = ScriptRequest {
                request: request.to_string(),
                repair: repair.take(),
                timeout_ms: SCRIPT_ROUND_MS,
                lang: llmproto::script::Lang::Rhai,
            };
            let failure = match self.host.script(&ask) {
                // No script: the model answered in words.
                Err(AnswerError::NoScript(reply)) if !reply.is_empty() => {
                    return self.say(id, request, reply);
                }
                Ok(answer) => match script::check(&answer, &self.host, &self.config) {
                    Ok(checked) => match guard::in_plan(&checked.plan, request) {
                        Some(why) => Some((answer.script, why)),
                        None => match (
                            self.told_only(request, &checked),
                            self.tier_of(request, &checked),
                        ) {
                            (Some(why), _) => Some((answer.script, why)),
                            (None, (Tier::Never, at)) => Some((
                                answer.script,
                                format!("{} may not be used in scripts here", at.join(", ")),
                            )),
                            (None, tier) => return self.propose(id, request, checked, tier),
                        },
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
        self.record(id, request, "", &format!("refused: {outcome}"));
        self.host.audit(&format!(
            "script {id} refused: {}",
            show::truncate(&outcome, 120)
        ));
        Reply::err(outcome)
    }

    fn say(&mut self, id: u64, request: &str, reply: String) -> Reply {
        self.worded = true;
        self.record(id, request, "", "answered in words");
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

    /// `new`: llm forgets the conversation. The action log and `undo` stay: what
    /// was done can still be undone.
    fn forget(&mut self) -> Reply {
        self.host.forget();
        self.clarifying = None;
        Reply::ok("A new conversation: earlier turns are forgotten; `log` and `undo` still work")
    }

    /// How many variables and functions the shell holds.
    pub fn sizes(&self) -> (usize, usize) {
        (self.scope.len(), self.functions.iter_functions().count())
    }
}

/// A line that does not run as Rhai is sent to the LLM if it reads like words: mostly
/// letters and no code punctuation. One starting with a Rhai keyword is Rhai with a
/// typo unless it is a sentence: "import the model on the attached disk" is words,
/// "let x be 5" and "print hello world" are code.
pub fn reads_like_words(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() || (RHAI_WORDS.contains(&words[0]) && !is_sentence(&words)) {
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

/// The fewest words a line led by a Rhai keyword needs to read as a sentence.
const SENTENCE_WORDS: usize = 4;

/// Whether `words` are a sentence: at least [`SENTENCE_WORDS`] of them, each
/// letters with at most an inner `'` or `-` and a trailing `,` `.` `?` or `!`.
/// A digit or an operator anywhere makes it code.
fn is_sentence(words: &[&str]) -> bool {
    words.len() >= SENTENCE_WORDS
        && words.iter().all(|word| {
            let word = word.trim_end_matches([',', '.', '?', '!']);
            !word.is_empty()
                && word.starts_with(char::is_alphabetic)
                && word
                    .chars()
                    .all(|c| c.is_alphabetic() || c == '\'' || c == '-')
        })
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
