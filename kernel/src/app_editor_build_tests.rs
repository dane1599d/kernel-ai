//! `test=app-editor-build` (P4 task 11, the editor on the default boot, C
//! components): Ctrl-B in the editor builds the C component whose folder the open
//! file is in, through the file door and the shell, and `undo` takes it back.
//!
//! The plan is the v0.1 system's pieces this needs: `fs` and `pkg` (`pkg-image`'s
//! step, so generation 1 is the image's packages), `cc`, `desktop`, the shell with
//! [`kabi::plan::SHELL_DESK_BUILD`]'s routes to `desktop` and `pkg`, and the
//! editor after it. The case writes a C app the image does not ship, `hello`, into
//! `/home/user/c/hello/` as `fs`'s kernel client, as `c-build` does; there is no
//! typed `cc build` here:
//!
//! 1. Ctrl-O and `/home/user/c/hello/hello.c`: the editor reads the file through
//!    the door;
//! 2. Ctrl-B: the editor asks the OS to build `hello` from the saved files, from
//!    the key's call, so it is the person's; `desktop` carries the request to the
//!    shell, which builds as it does for a typed line: `local-next` gives 0.0.1,
//!    `cc` builds, `pkg` publishes into `local`, installs generation 2 and starts
//!    the app. The editor's status says so;
//! 3. the app runs 0.0.1 (its `init` logs a line);
//! 4. `undo`, typed at the shell: packages go back to generation 1, which ends the
//!    app, as it does after a typed build. The build is in the same log, one entry.
//!
//! Each step of the host's keys (a stage in the case's table, sent when this test
//! logs `ready for keys`) is waited for in the editor's own status, so a slow
//! guest (TCG) only takes longer.

use crate::app_editor_files_tests::{
    DESKTOP, ask_to_open, click_into_the_code, desktop_and_editor, shell, showing_file,
};
use crate::app_editor_tests::{WAIT_MS, cursor, status, until};
use crate::cc_build_tests::{BUILD_MS, HELLO, SYNC_MS, wait_up, write_sources};
use crate::pkg_tests::{Step, wait_for};
use crate::{log, threads};
use alloc::format;
use alloc::string::String;

/// `pkg`'s image step and `fs`, the model, `cc`, `desktop`, the built app's line,
/// the shell with `updated` just before it (started on demand), and the editor.
pub fn plan() -> String {
    format!(
        "{}\n{}\n{}\n{DESKTOP}\n{HELLO}\n{}\n{}",
        crate::pkg_tests::plan(Step::Image),
        crate::llm::plan_line(),
        kabi::plan::CC,
        kabi::plan::with_updated(kabi::plan::SHELL_DESK_BUILD, kabi::plan::ON_DEMAND),
        kabi::plan::EDITOR,
    )
}

/// Where the sources are: the folder `cc build hello` reads.
const SOURCE: &str = "hello.c";

/// The editor opens the source `write_sources` made.
fn open_the_source() -> Result<(), &'static str> {
    click_into_the_code();
    ask_to_open();
    until("the source to open in the editor", WAIT_MS, |said| {
        showing_file(said, SOURCE) && cursor(said) == Some((1, 0))
    })?;
    log!("app-editor-build: Ctrl-O opened /home/user/c/hello/hello.c");
    Ok(())
}

/// Ctrl-B, and the wait for the report in the editor's status.
fn build_from_the_editor() -> Result<(), &'static str> {
    log!("app-editor-build: ready for keys");
    let said = until("the editor to say it built hello", BUILD_MS, |said| {
        said.contains("built hello 0.0.1 (") && said.contains("as generation 2. undo")
    })?;
    log!(
        "app-editor-build: Ctrl-B built hello 0.0.1 from the editor, and pkg installed it as generation 2: {}",
        said.replace('\n', "; ")
    );
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    wait_for(&format!("case/{}.done", Step::Image.name()))
        .map_err(|_| "app-editor-build: pkg did not finish its image step")?;
    // The marker is durable once written, but `pkg` syncs it next, and an action of
    // the kernel's begun first would make that sync Busy.
    threads::sleep_ms(SYNC_MS);
    write_sources()?;
    desktop_and_editor()?;
    log!(
        "app-editor-build: the editor says {}",
        status()?.replace('\n', "; ")
    );
    open_the_source()?;
    build_from_the_editor()?;
    let label = wait_up("0.0.1")?;
    log!("app-editor-build: hello 0.0.1 runs as {label}");
    let undone = shell("undo")?;
    if !undone.contains("packages back to generation 1") {
        log!("app-editor-build: undo: {undone:?}");
        return Err("app-editor-build: undo did not switch back to generation 1");
    }
    log!("app-editor-build: undo took the editor's build back to generation 1");
    Ok(())
}
