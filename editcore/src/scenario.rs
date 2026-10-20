//! The M9b exit test, as data. xtask runs it on the host first (real builds, the
//! store and ledger code standing in for the kernel, notes on wasmi), then the
//! kernel runs it in QEMU (`test=edit`) through the same functions the terminal
//! calls. Every check reads the store, the ledger or the outbox, never a print.
//!
//! A. notes 2 builds state and its source opens in the editor. S. Only the kernel
//! can drive the edit service; the kernel refuses an upload that no build of the
//! build service backs, and relays for it only control requests that change
//! nothing. B. A working edit is built, tried on copies, declined once, then
//! applied live with its state; it builds in the background, and BuildDone asks
//! `status` until it is done. C. A compile error changes nothing. D. An index out of
//! bounds is built, applied, and rolls back by itself; before it is applied, the
//! edit service is killed and the kernel's journal gives the new one the session
//! back. E. A host restart comes back on the upload. K. `:revert` goes back to
//! version 2.

use alloc::string::String;
use alloc::vec::Vec;
use kabi::Error;
use livecore::Reason;
use livecore::scenario::{self as live, Count, Driver as LiveDriver, Expect};
use livecore::store::Component;
use livecore::uploads::{Outcome, SyncJob};

mod run;

pub use run::run;

/// Part B's edit: version e1 says it was edited live.
pub const EDIT_ABOUT: &str = r#"s |schema {SCHEMA}")|schema {SCHEMA}, edited live")|"#;
/// Part C's edit: adds a string to a number, which does not compile.
pub const EDIT_COMPILE: &str = r#"s |next_id += 1;|next_id += "1";|"#;
/// Part D's edit: `count` reads one past the end of the notes.
pub const EDIT_TRAP: &str =
    "s |self.state.notes.len())|self.state.notes[self.state.notes.len()].id)|";
/// Where each edit lands in `components/notes/src/lib.rs`.
pub const EDIT_LINES: [(&str, usize); 3] = [(EDIT_ABOUT, 141), (EDIT_COMPILE, 40), (EDIT_TRAP, 58)];

/// How `:apply` went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// It showed the view and asked; `outcome` is what the answer led to.
    Asked { view: String, outcome: String },
    /// It refused before asking.
    Refused(String),
}

/// What the edit scenario can do and see, beyond the live one.
pub trait Driver: LiveDriver {
    /// A line typed in edit mode: an editor command, or `:edit` / `:quit`.
    fn edit(&mut self, line: &str) -> Result<String, String>;
    /// `:apply notes`, answered `answer` if it asks.
    fn apply(&mut self, answer: &str) -> Result<Applied, String>;
    /// `:revert notes [n]`.
    fn revert(&mut self, args: &str) -> Result<String, String>;
    /// A copy of the store's record of notes.
    fn component(&mut self) -> Component;
    /// Builds sent to the build service so far.
    fn builds(&mut self) -> u64;
    /// Waits until the outbox is empty; the jobs git acknowledged.
    fn settle_sync(&mut self) -> Result<Vec<SyncJob>, String>;
    /// The processes holding PUBLISH and COMPONENT; None where there are none.
    fn holders(&mut self) -> Option<(Vec<String>, Vec<String>)>;
    /// A request to the edit service from some other process.
    fn spoof_edit(&mut self, request: &str) -> Result<String, String>;
    /// The kernel puts boot version `label`'s own artifact as an upload.
    fn put_boot_artifact(&mut self, label: &str) -> Result<(), Error>;
    /// Has the kernel send `ctl <request>` to notes as COMP_CONTROL does for the
    /// edit service; the reply.
    fn relay(&mut self, request: &str) -> Result<String, Error>;
    /// Kills the edit service; returns once a new one serves.
    fn restart_editor(&mut self) -> Result<(), String>;
    /// What the kernel said when it last gave a restarted edit service the session.
    fn restored(&mut self) -> Option<String>;
    /// Waits `ms` milliseconds; BuildDone waits between its status checks.
    fn wait(&mut self, ms: u64);
}

#[derive(Clone, Copy, Debug)]
pub enum Step {
    Live(live::Step),
    /// A line typed in edit mode; the reply holds every expectation.
    Edit(&'static str, &'static [Expect]),
    /// `status` until no build runs; the last build's answer holds every
    /// expectation.
    BuildDone(&'static [Expect]),
    /// `:apply`, answered; the view and then the outcome.
    Apply {
        answer: &'static str,
        view: Expect,
        outcome: Expect,
    },
    /// `:apply` refuses before asking.
    ApplyRefused(Expect),
    Revert(&'static str, Expect),
    /// The uploads stored, with their bases, and the next label.
    Uploads(&'static [(&'static str, &'static str)], &'static str),
    /// Uploads the store refused.
    UploadsRefused(u64),
    /// Remembers the store, the ledger, the fetches, the host and the outbox.
    Snapshot,
    /// All of those are as the snapshot left them.
    Unchanged,
    /// Builds sent to the build service so far.
    Builds(u64),
    /// The last job git acknowledged.
    Synced(&'static str, Outcome),
    /// Only `edit` holds PUBLISH and only `notes` COMPONENT (not on the host).
    Holders,
    SpoofEdit(&'static str, Expect),
    /// The kernel refuses a boot artifact put as an upload: no build backs it.
    PutRefused(&'static str),
    /// The kernel will not relay these control requests for the edit service.
    RelayRefused(&'static [&'static str]),
    /// The edit service is killed, and the supervisor starts it again.
    RestartEditor,
    /// What the kernel said when it replayed the journal into the new service.
    Restored(Expect),
}

use Count::*;
use Expect::*;
use Step::*;
use live::Step::{Active, Bad, Call, Counts, Fetches, Part, Restart, RolledBack, SameHost};

pub const STEPS: &[Step] = &[
    Live(Part(
        "A: notes 2 builds state; its source opens in the editor",
    )),
    Live(Call("add buy milk", Exact("ok 1"))),
    Live(Call("add walk", Exact("ok 2"))),
    Live(Active("2", 2)),
    Uploads(&[], "e1"),
    Edit(":edit notes", &[StartsWith("ok editing notes 2: 5 files")]),
    Edit(
        "find schema {SCHEMA}",
        &[Contains("components/notes/src/lib.rs:141:")],
    ),
    Live(Part(
        "S: only the kernel drives the editor and fills the store",
    )),
    SpoofEdit(
        "cmd build",
        Exact("err the edit service answers only the kernel"),
    ),
    Holders,
    PutRefused("2"),
    UploadsRefused(1),
    RelayRefused(&["swap e1", "swap 2 force", "rollback", "reset", "try ../x"]),
    Uploads(&[], "e1"),
    Live(Part("B: a working edit, declined once, then applied live")),
    Edit(
        EDIT_ABOUT,
        &[Contains("lib.rs:141:"), Contains("edited live")],
    ),
    Edit("build", &[Exact("ok building e1 from 2")]),
    // Answered while the host compiles.
    Edit("p 141", &[Contains("edited live")]),
    Edit("status", &[Exact("ok notes 2: edited; building e1 from 2")]),
    BuildDone(&[StartsWith("ok built e1 from 2 (commit ")]),
    Edit(
        "try",
        &[
            StartsWith("ok try e1: import ok, self-test ok, health ok, export ok, "),
            Contains(", reverse ok"),
        ],
    ),
    Uploads(&[("e1", "2")], "e2"),
    Builds(1),
    Live(Counts(&[(Begins, 0), (Generation, 0)])),
    Apply {
        answer: "no",
        view: Contains("components/notes/src/lib.rs line 141: 1 removed, 1 added"),
        outcome: Exact("not applied"),
    },
    Live(Counts(&[(Begins, 0)])),
    Apply {
        answer: "yes",
        view: Contains("schema {SCHEMA}, edited live\")"),
        outcome: Exact("ok probation e1 (3 calls)"),
    },
    Live(Call("count", Exact("ok 2 notes, 3 words"))),
    Live(Call("get 1", Exact("ok buy milk"))),
    Live(Call("about", Exact("ok notes e1 schema 2, edited live"))),
    Live(Active("e1", 2)),
    Live(Counts(&[(Commits, 1), (Generation, 1), (Served, 3)])),
    Live(SameHost),
    Synced("e1", Outcome::Commit),
    ApplyRefused(Exact("nothing new: e1 is active")),
    Live(Part("C: a compile error changes nothing")),
    Edit(":edit notes", &[StartsWith("ok editing notes e1: 5 files")]),
    Edit(EDIT_COMPILE, &[Contains("lib.rs:40:")]),
    // Off line 40 first, so only the failed build can bring the cursor back.
    Edit(
        "open Cargo.toml",
        &[StartsWith("ok components/notes/Cargo.toml, ")],
    ),
    Snapshot,
    Edit("build", &[Exact("ok building e2 from e1")]),
    BuildDone(&[
        StartsWith("err build failed; the cursor is on the first error"),
        Contains("error[E0277]"),
        Contains("components/notes/src/lib.rs:40:"),
    ]),
    Edit(
        "p",
        &[StartsWith("ok components/notes/src/lib.rs lines 40 to ")],
    ),
    Unchanged,
    Builds(2),
    Live(Part("D: an index out of bounds rolls back by itself")),
    Edit("reset", &[Exact("ok back to notes e1 as opened")]),
    Edit(EDIT_TRAP, &[Contains("lib.rs:58:")]),
    Edit("build", &[Exact("ok building e2 from e1")]),
    BuildDone(&[StartsWith("ok built e2 from e1 (commit ")]),
    // The edit service dies after the build; the journal heard of e2 from the
    // store, so the next line brings the session back, built.
    RestartEditor,
    Edit(
        "p 58",
        &[Contains("self.state.notes[self.state.notes.len()]")],
    ),
    Restored(Exact(
        "the edit service restarted; restored notes e1 with 1 edit",
    )),
    Edit(
        "build",
        &[Exact(
            "err e2 was built from this text already; :apply runs it",
        )],
    ),
    Edit(
        "undo",
        &[StartsWith("ok undone; components/notes/src/lib.rs:1: ")],
    ),
    Edit("p 58", &[Contains("self.state.notes.len())")]),
    Apply {
        answer: "yes",
        view: Contains("components/notes/src/lib.rs line 58: 1 removed, 1 added"),
        outcome: Exact("ok probation e2 (3 calls)"),
    },
    // e2 traps on the request; e1 already answered it.
    Live(Call("count", Exact("ok 2 notes, 3 words"))),
    Live(Active("e1", 2)),
    Live(Counts(&[
        (Rollbacks, 1),
        (Because(Reason::Trap), 1),
        (Commits, 1),
        (Begins, 2),
    ])),
    Live(Bad("e2", true)),
    // The trap names the line of part D's edit.
    Live(RolledBack(
        "e2",
        StartsWith("panicked at components/notes/src/lib.rs:58:"),
    )),
    // :fault opens e2 at that line.
    Edit(
        ":fault notes",
        &[StartsWith(
            "ok editing notes e2 at components/notes/src/lib.rs:58: 5 files",
        )],
    ),
    Edit("p", &[Contains("self.state.notes[self.state.notes.len()]")]),
    Live(Call("about", Exact("ok notes e1 schema 2, edited live"))),
    Synced("e2", Outcome::Rollback(Reason::Trap)),
    ApplyRefused(StartsWith("e2 rolled back before (trap)")),
    Live(Counts(&[(Begins, 2)])),
    Builds(3),
    Live(Part("E: a host killed while on an upload comes back on it")),
    Live(Restart),
    Live(Call("about", Exact("ok notes e1 schema 2, edited live"))),
    Live(Call("count", Exact("ok 2 notes, 3 words"))),
    Live(Fetches(&[("e1", 3)])),
    Live(Counts(&[(Restores, 2)])),
    Live(Part("K: revert goes back to version 2")),
    Revert("notes", Exact("ok probation 2 (3 calls)")),
    Live(Call("count", Exact("ok 2 notes, 3 words"))),
    Live(Call("get 1", Exact("ok buy milk"))),
    Live(Call("about", Exact("ok notes 2 schema 2"))),
    Live(Active("2", 2)),
    Live(Counts(&[(Commits, 2), (Generation, 2), (Rejected, 0)])),
    Synced("2", Outcome::Commit),
    Uploads(&[("e1", "2"), ("e2", "e1")], "e3"),
    UploadsRefused(1),
    Edit(":quit", &[StartsWith("ok")]),
];
