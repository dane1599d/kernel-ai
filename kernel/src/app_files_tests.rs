//! `test=app-files` (P4 task 5's exit case): the agent changes the person's files
//! and the Files window shows each state. The Files app runs on the desktop, its
//! effects answered by the shell, on a `kai-data` disk; the mock model answers
//! each request line with its call (`xtask/src/mock_llm.rs`).
//!
//! The OS's own rows change the files, so their tiers and `undo` are the OS's: a
//! create, a move and a trash are all logged and undoable, so each runs at once and
//! asks nothing. The window is the app's, so the agent has it show the folder
//! (`files_reveal`, which lists the folder again) and asks it what it lists
//! (`files_select` answers only for a name the window lists):
//!
//! 1. "create notes.txt saying hello": the window lists `notes.txt`;
//! 2. "move notes.txt to plan.txt": it lists `plan.txt` and no longer `notes.txt`;
//! 3. "trash plan.txt": it lists neither;
//! 4. `undo`: `plan.txt` is back; `undo` again: `notes.txt` is, and `plan.txt` is not;
//! 5. "open the docs folder": a second folder, which the case made before the agent
//!    started (the agent has no row that makes one), and the window lists its
//!    `todo.txt` and not `notes.txt`; showing `notes.txt` again brings home back.
//!
//! The fifth step is a row that moves the folder shown (`files_open`): the host
//! refuses a read-only row that changes the app's checkpoint, so the folder is not in
//! the app's (see `apps/files-window`), and this step proves it on a boot.
//!
//! The shell's log (`log`) shows the move, the trash and each undo, asked right
//! after each: it keeps its last 64 entries, and the polling adds some.

use crate::display::claim;
use crate::files_tests::ask;
use crate::{console, ipc, log, process, threads};
use alloc::format;
use alloc::string::String;
use fsproto::{How, Reply, Request};
use kabi::handle::Rights;

const NAME: &str = "files-window";

/// The plan lines this case adds, after the shell's `cc` and before the shell,
/// whose route to `desktop` (`SHELL_DESK_FILES`) the app's effects take: `desktop`
/// as the other agent cases run it, and the Files app's own line.
pub fn plan() -> String {
    format!(
        "{}\n{}",
        crate::agent_desk_tests::DESK,
        kabi::plan::FILES_WINDOW
    )
}

const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app, paint its first frame and have its first
/// listing answered, under TCG.
const OPEN_MS: u64 = 12_000;
/// How long the window may take to show a change the agent asked for: the shell
/// runs the app's listing and the answer comes back through comphost.
const SHOW_MS: u64 = 120_000;
const POLL_MS: u64 = 1_000;

/// The requests the mock model answers (`xtask/src/mock_llm.rs`).
const CREATE: &str = "create notes.txt saying hello";
const MOVE: &str = "move notes.txt to plan.txt";
const TRASH: &str = "trash plan.txt";
const SHOW: &str = "show notes.txt in the files app";
const PICK_NOTES: &str = "pick notes.txt in the files app";
const PICK_PLAN: &str = "pick plan.txt in the files app";
const OPEN_DOCS: &str = "open the docs folder in the files app";
const PICK_TODO: &str = "pick todo.txt in the files app";

/// The shell's raw reply to `line`.
fn reply_to(line: &str) -> Result<String, &'static str> {
    let reply = ipc::call("shell", line.as_bytes(), CALL_MS)
        .map_err(|_| "app-files: the shell did not reply")?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// Runs `line`, whose answer must be exactly `done`.
fn does(line: &str, done: &str) -> Result<(), &'static str> {
    let said = reply_to(line)?;
    if said != done {
        log!("app-files: {line:?} answered {said:?}, not {done:?}");
        return Err("app-files: a change did not run as it should");
    }
    log!("app-files: {line:?} answered {done:?}");
    Ok(())
}

/// The shell's log (`log`) has an entry with `needed` in it. It keeps only its
/// last entries, and the agent's polling adds some, so this is asked right after
/// the change it shows.
fn logged(needed: &str) -> Result<(), &'static str> {
    let history = reply_to("log")?;
    if !history.contains(needed) {
        log!(
            "app-files: the log has no {needed:?}: {:?}",
            history.replace('\n', "; ")
        );
        return Err("app-files: the log does not show a change or its undo");
    }
    log!("app-files: the log shows {needed:?}");
    Ok(())
}

/// Whether the window lists the name `pick` selects: `files_select` is `ok` for a
/// name in the folder the window shows, and says there is none here for another.
/// Anything else is the app not answering, which no wait will mend.
fn lists(pick: &str) -> Result<bool, &'static str> {
    let said = reply_to(pick)?;
    if said.starts_with("ok ") && said.contains("Selected ") {
        return Ok(true);
    }
    if said.contains("There is no ") {
        return Ok(false);
    }
    log!("app-files: {pick:?} answered {said:?}");
    Err("app-files: the Files app did not answer a select")
}

/// Has the window list the folder again, then waits until it lists `notes` and
/// `plan` as given: the shell answers the listing the app asks for, so the window
/// takes a moment to show it.
fn shows(notes: bool, plan: bool, what: &'static str) -> Result<(), &'static str> {
    let said = reply_to(SHOW)?;
    if !(said.starts_with("ok ") && said.contains("Showing notes.txt")) {
        log!("app-files: {SHOW:?} answered {said:?}");
        return Err("app-files: the Files app did not show the folder");
    }
    for _ in 0..SHOW_MS / POLL_MS {
        if lists(PICK_NOTES)? == notes && lists(PICK_PLAN)? == plan {
            log!("app-files: the window lists {}", describe(notes, plan));
            return Ok(());
        }
        threads::sleep_ms(POLL_MS);
    }
    log!(
        "app-files: after {what} the window never listed {}",
        describe(notes, plan)
    );
    Err(what)
}

fn describe(notes: bool, plan: bool) -> &'static str {
    match (notes, plan) {
        (true, true) => "notes.txt and plan.txt",
        (true, false) => "notes.txt only",
        (false, true) => "plan.txt only",
        (false, false) => "neither file",
    }
}

/// `/home/user/docs/todo.txt`, in one action of the kernel's own: the agent has no
/// row that makes a folder.
fn make_docs() -> Result<(), &'static str> {
    let home = kabi::fs::HOME_USER;
    let done = |reply: Reply| match reply {
        Reply::Done => Ok(()),
        _ => Err("app-files: fs refused a request"),
    };
    done(ask(home, Request::Begin { action: 9_000 })?)?;
    let open = Request::Open {
        path: "docs".into(),
        rights: (Rights::READ | Rights::ENUMERATE).bits(),
        how: How::Mkdir,
    };
    if !matches!(ask(home, open)?, Reply::Opened(..)) {
        return Err("app-files: the docs folder cannot be made");
    }
    let open = Request::Open {
        path: "docs/todo.txt".into(),
        rights: (Rights::READ | Rights::WRITE).bits(),
        how: How::CreateFile,
    };
    let Reply::Opened(_, file) = ask(home, open)? else {
        return Err("app-files: docs/todo.txt cannot be made");
    };
    let write = Request::Write {
        off: 0,
        data: b"todo".to_vec(),
    };
    done(ask(file, write)?)?;
    match ask(home, Request::Commit)? {
        Reply::Committed { .. } => Ok(()),
        _ => Err("app-files: the docs folder did not commit"),
    }
}

/// The agent opens the second folder: the row moves the folder shown, which only a
/// row that is read-only and leaves the checkpoint alone may do. The window then
/// lists the folder's file and not the home folder's.
fn visits_docs() -> Result<(), &'static str> {
    let said = reply_to(OPEN_DOCS)?;
    if !(said.starts_with("ok ") && said.contains("Opening docs")) {
        log!("app-files: {OPEN_DOCS:?} answered {said:?}");
        return Err("app-files: the Files app did not open a second folder");
    }
    for _ in 0..SHOW_MS / POLL_MS {
        if lists(PICK_TODO)? && !lists(PICK_NOTES)? {
            log!("app-files: the window lists docs/todo.txt and not notes.txt");
            return Ok(());
        }
        threads::sleep_ms(POLL_MS);
    }
    Err("app-files: the window never listed the second folder")
}

pub fn check() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && !process::named(NAME).is_empty() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        return Err("app-files: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    make_docs()?;

    does(CREATE, "ok Done: file notes.txt created (was absent). undo")?;
    shows(
        true,
        false,
        "app-files: the window did not list the new file",
    )?;
    log!("app-files: the Files window lists the file the agent made");

    does(
        MOVE,
        "ok Done: file notes.txt moved to plan.txt (was there). undo",
    )?;
    logged("move notes.txt to plan.txt: ran")?;
    shows(
        false,
        true,
        "app-files: the window did not list the moved file",
    )?;
    log!("app-files: after the move the window lists plan.txt and no longer notes.txt");

    does(
        TRASH,
        "ok Done: file plan.txt in the trash (was there). undo",
    )?;
    logged("trash plan.txt: ran")?;
    shows(
        false,
        false,
        "app-files: the window did not drop the trashed file",
    )?;
    log!("app-files: after the trash the window lists neither file");

    does("undo", "ok Undone: files back as they were")?;
    logged("undo of #")?;
    shows(
        false,
        true,
        "app-files: the window did not list the restored file",
    )?;
    does("undo", "ok Undone: files back as they were")?;
    logged("undo of #")?;
    shows(
        true,
        false,
        "app-files: the window did not list the moved-back file",
    )?;

    visits_docs()?;
    shows(
        true,
        false,
        "app-files: the window did not come back to the home folder",
    )?;
    if lists(PICK_TODO)? {
        return Err("app-files: the window still lists the second folder's file");
    }
    log!("app-files: the window went to a second folder and came back to the home folder");
    log!(
        "app-files: the agent made, moved and trashed a file at once, the Files window showed each state and a second folder, and undo put both back; the log showed each change and each undo"
    );
    Ok(())
}
