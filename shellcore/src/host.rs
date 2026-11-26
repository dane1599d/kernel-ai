//! What the shell needs from the operating system ([`Host`]), and the values it
//! passes through it. Re-exported at the crate root.

use crate::{
    LogFailed, appbuild, appjob, apps, ccbuild, outbox, standing, state, targets, ui, waiting,
};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::sched::{SchedStats, TrialRequest};
use llmproto::act::{ActError, ActRequest, Call};
use llmproto::chat::Completion;
use llmproto::script::{Answer, AnswerError, Lang, ScriptRequest};
use llmproto::tune::TuneRequest;

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
    /// Paths under home whose last part holds this word, case aside.
    Find(&'a str),
}

/// A change the `fs` server makes as one transaction, which `undo` restores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsChange<'a> {
    Create {
        path: &'a str,
        text: &'a str,
    },
    Write {
        path: &'a str,
        text: &'a str,
    },
    /// `:save`'s file at `c>` (ADR 0012 D4): `path` made or replaced, its
    /// folder `folder` (one beneath `/home/user`) made first when missing.
    Save {
        folder: &'a str,
        path: &'a str,
        text: &'a str,
    },
    Move {
        from: &'a str,
        to: &'a str,
    },
    Trash(&'a str),
    Restore(&'a str),
    EmptyTrash,
}

/// Why the `fs` server did not restore a transaction. There is no "put back
/// anyway": `fs` never restores over a later change, so asking would offer
/// what no yes can do (bug-hunt-6 H6-22).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsRefused {
    Failed(String),
    /// Nothing was put back: everything the transaction changed has changed
    /// since. Asking again would find the same, so no one is told to.
    Kept(String),
}

/// A restore `fs` committed: its generation, which a restore of it undoes, and
/// how many of the transaction's files it left as they are, because a later
/// change touched them (`fs` keeps those; a file's old version goes beside it).
/// One that left some is still a change that landed, so it is logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FsUndone {
    pub generation: u64,
    pub left: usize,
}

impl FsUndone {
    /// What `fs`'s restore rows, committed as `generation`, mean for an undo.
    /// `fs` never restores over a later change; it leaves those. A restore that
    /// put any of the rest back landed, and is the undo: asking again would
    /// restore it again, and each time write one more `name (restored)` copy
    /// (bug-hunt-6 H6-22). One that put nothing back is refused. Only files
    /// count as left: a later change that only added or removed a name in a
    /// folder leaves the folder too, though every file is back (#1442's review).
    pub fn of(generation: u64, rows: &[(u64, fsproto::Outcome)]) -> Result<Self, FsRefused> {
        use fsproto::Outcome;
        let left = rows
            .iter()
            .filter(|(_, o)| *o == Outcome::Left { folder: false })
            .count();
        let restored = rows.iter().any(|(_, o)| !matches!(o, Outcome::Left { .. }));
        match (left, restored) {
            (_, true) => Ok(FsUndone { generation, left }),
            _ if rows.is_empty() => Ok(FsUndone { generation, left }),
            (0, false) => Err(FsRefused::Kept(String::from(
                "it changed since, and fs never restores over a later change, so it stays as it is now",
            ))),
            (n, false) => Err(FsRefused::Kept(format!(
                "{n} file(s) changed since, and fs never restores over a later change, so they stay as they are now"
            ))),
        }
    }
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
    /// Whether the person pressed Stop during the request being served. In the
    /// OS it stays true for the rest of that request (`kabi::call::INTERRUPTED`).
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
    /// [`Host::act`], handing `serve` what the apps ask while the call is in
    /// flight ([`waiting`]): a read it answers goes back to the app at once,
    /// and one it does not (`None`) waits for the line, in its app's order. A
    /// host that cannot wait off its main thread just asks.
    fn act_serving(
        &self,
        request: &ActRequest,
        serve: &mut dyn FnMut(waiting::Waiting<'_>) -> Option<waiting::Served>,
    ) -> Result<Call, ActError> {
        let _ = serve;
        self.act(request)
    }
    /// Keeps a finished model turn in llm's conversation memory (P1 task 10): the
    /// user's line, the call as `act` returned it, and the result line.
    fn remember(&self, _line: &str, _call: &str, _result: &str) {}
    /// Forgets the conversation (`new`), or says why llm did not.
    fn forget(&self) -> Result<(), String> {
        Ok(())
    }
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
    /// Restarts the computer (`POWER_REBOOT`, backlog item 13) after the
    /// restart's Allow. On the OS it does not come back unless the kernel said
    /// no; a test host records it and comes back with `Ok`.
    fn power_reboot(&self) -> Result<(), String> {
        Err(String::from("this host cannot restart the computer"))
    }
    /// The user's appearance as `kai-appearance 1` text (P4 task 8): `None`
    /// with no file, and why not when it could not be read (a failed read is no
    /// proof the file is absent, so nothing is written over it).
    fn appearance_read(&self) -> Result<Option<String>, String> {
        Ok(None)
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
    /// Seconds since 1970 UTC from the kernel's clock (`CLOCK_NOW`), or `None`
    /// where the firmware gave no date.
    fn unix_seconds(&self) -> Option<u64> {
        None
    }

    /// Installed package `package`'s `kai-actions 1` answer, read by `pkg`
    /// from its stored artifact: an app's rows before it first runs.
    fn installed_rows(&self, _package: &str) -> Result<String, String> {
        Err(String::from("there is no pkg service here"))
    }
    /// Whether `pkg` has `package` installed now, asked with a short wait:
    /// `Err` when it gave no clear answer.
    fn package_installed(&self, package: &str) -> Result<bool, String> {
        match self.installed_rows(package) {
            Ok(_) => Ok(true),
            Err(why) if why.ends_with(" is not installed") => Ok(false),
            Err(why) => Err(why),
        }
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
    /// A held send that is due (`outbox`): `send.body` posted to `send.path`
    /// on `send.host`, once, never retried and never redirected.
    ///
    /// Fail-closed: `Ok` with the answer's status code and reason (`200 OK`,
    /// `403 Forbidden`) for any HTTP answer at all (a 403 or a 500 too: the
    /// site got the body), and `Err` only when no answer came. `maybe_sent` is false only for a failure before any byte
    /// of the request was written (no address, no connection, the TLS
    /// handshake refused); anything after that may have gone out.
    /// Whether a send's card fits beside the open Kai pane, the history
    /// uncovered (`desktop`'s `panel send-fits`): where a send typed at the
    /// desk may be answered on its card. No desktop, no.
    fn desk_send_fits(&self) -> bool {
        false
    }
    /// Gives `desktop` the text of the send whose card `id` goes up now, for
    /// the Kai pane (`deskcore::sendview`, `panel preview <id>`).
    fn desk_preview(&self, _id: u64, _text: &str) -> Result<(), String> {
        Err(String::from("there is no desktop here"))
    }
    fn net_send(&self, _send: &outbox::Send) -> Result<String, outbox::SendFailed> {
        Err(outbox::SendFailed {
            why: String::from(NO_NET),
            maybe_sent: false,
        })
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
    /// Writes the whole action log to `/system/log`, after any line that changed
    /// it, in the same commit as the line's files: [`LogFailed::NotLanded`]
    /// when that commit was refused.
    fn log_write(&self, _text: &str) -> Result<(), LogFailed> {
        Ok(())
    }
    /// Restores what transaction `txn` replaced, as one new transaction: its number,
    /// so `undo` again can redo, and how many files it left because a later change
    /// touched them. Refused when it would restore none of them.
    fn fs_undo(&self, _txn: u64) -> Result<FsUndone, FsRefused> {
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
    /// rule lines), read once at start; `None` when there are none (P2 task 12),
    /// and why not when it could not be read.
    fn rules_read(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    /// Writes the whole rules file, after a rule is added or removed.
    fn rules_write(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    /// The keep-running list as `/system/state/keep` holds it, and why not
    /// when it could not be read.
    fn keep_read(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn keep_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("there is no /system/state here"))
    }
    /// `keep <app> on|off` to the supervisor: app `app` naps or is kept running.
    fn keep_running(&self, _app: &str, _on: bool) -> Result<(), String> {
        Err(String::from("apps cannot be kept running here"))
    }
    /// The restart window as `/system/state/restart/window` holds it
    /// (`kai-updates 1`), which `updated` follows: `None` with no file, and why
    /// not when it could not be read.
    fn restart_read(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn restart_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("there is no /system/state here"))
    }
    /// The rows taken back from apps' effects, as `/system/state/withheld`
    /// holds them (`kai-withheld 1`, P4 task 7): `None` with no file, and
    /// why not when it could not be read.
    fn withheld_read(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    /// Writes the whole withheld file, before a change takes effect.
    fn withheld_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("grants cannot be revoked here"))
    }
    /// The apps' network grants as `/system/state/netgrants` holds them
    /// (`kai-grants 1` host lines, P5 task 13), and why not when it could
    /// not be read.
    fn net_grants_read(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    /// Writes the whole network grants file, before a change is told.
    fn net_grants_write(&self, _text: &str) -> Result<(), String> {
        Err(String::from("network grants cannot be kept here"))
    }
    /// The standing instructions as `/system/state/standing` holds them
    /// (`kai-standing 1`), read once at start (P3 task 11), and why not when
    /// it could not be read.
    fn standing_read(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    /// Writes the whole standing-instruction store, after one changes.
    fn standing_write(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    /// Gives each process its class and quota (P3 task 12), through the
    /// supervisor's PROC_CLASS_SET and PROC_QUOTA_SET; every one's settings from
    /// before, so `undo` can put them back. With `cap` (a switch, #517) a
    /// setting's `mem` is a ceiling: a quota above it is lowered, one at or
    /// under it is left, and only a lowered one has a `mem` from before. Without
    /// it (an undo) `mem` is set exactly.
    fn profile_apply(
        &self,
        _settings: &[kaiprofile::Setting],
        _cap: bool,
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
