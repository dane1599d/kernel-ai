//! `test=app-build` and `test=app-build-deny` ("Kai builds apps", end to end
//! with the mock model): a typed line builds an app in the background, its
//! install card wakes the terminal by itself, and the person's key on
//! `desktop`'s card decides.
//!
//! 1. "build me an app called shopping": the mock model's act answer is
//!    `app_build("shopping", "a shopping list")` (`xtask/src/mock_llm/acts.rs`).
//!    llm runs the `c-app-1` round as a job on its server backend, so the line
//!    answers at once ("Building shopping"); the mock answers the round with
//!    the card's worked example (`xtask/src/mock_llm/app.rs`). Between lines
//!    the shell keeps it as `/home/user/apps/shopping/main.c`, `cc` builds
//!    0.0.1 (`local-next`), and the trial passes. Nothing is installed.
//! 2. The build's end leaves its install card waiting: the shell says so
//!    (CARD_READY), and the terminal, blocked at its prompt, takes it and posts
//!    it to `desktop`'s card: the card for `app_install("shopping", "0.0.1",
//!    ...)`. Packages are still generation 1.
//! 3. `app-build`: the host's `y` on the card (the case logs `ready for
//!    keys`) is the person's Allow. `pkg` installs the build's exact bytes from
//!    `local` as generation 2 and starts the app on its `start=pkg` line
//!    ([`SHOPPING`]), and its window, "Shopping", opens. `undo` puts packages
//!    back to generation 1, which ends the app; the source stays.
//! 4. `app-build-deny`: the host's `n` is the person's Deny. Nothing starts,
//!    packages stay at generation 1, and the source stays.
//!
//! The host checks comphost's, `desktop`'s and `pkg`'s lines.

use crate::agent_desk_tests::{DESK, KEYS_MS, OPEN_MS};
use crate::cc_build_tests::{BUILD_MS, SYNC_MS};
use crate::display::claim;
use crate::files_tests::ask;
use crate::pkg_tests::{Step, generation, wait_for};
use crate::{ask_box, ipc, log, process, terminal, threads};
use alloc::format;
use alloc::string::String;
use fsproto::{How, Reply, Request};
use kabi::handle::Rights;

/// The line, as the mock model answers it.
// No telling word in it: "a shopping list app" opens the build on Deny, since
// consent reads the noun "list" as asking to be told (#1214, held).
const LINE: &str = "build me an app called shopping";
const NAME: &str = "shopping";
/// The built app's plan line: a window app `pkg` starts, as `c-build`'s
/// `hello` is. With no `start=pkg` line, the supervisor refuses `pkg`'s
/// `run` for it.
const SHOPPING: &str = "shopping program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 start=pkg";
/// What the mock's source holds, read back at the end.
const SOURCE_MARK: &str = "const char TITLE[] = \"Shopping\";";
/// Longest wait for a step that is not a build: `desktop`, the card's answer,
/// the app's start or end, under TCG too.
const STEP_MS: u64 = 120_000;
/// After the Deny: how long nothing may start.
const QUIET_MS: u64 = 10_000;
const POLL_MS: u64 = 50;

/// `pkg`'s image step and `fs`, the model, `cc`, `desktop` with its card, the
/// built app's line, and the shell with `updated` just before it, started on
/// demand, with routes to `desktop` and `pkg`.
pub fn plan() -> String {
    format!(
        "{}\n{}\n{}\n{DESK}\n{SHOPPING}\n{}",
        crate::pkg_tests::plan(Step::Image),
        crate::llm::plan_line(),
        kabi::plan::CC,
        kabi::plan::with_updated(kabi::plan::SHELL_DESK_BUILD, kabi::plan::ON_DEMAND),
    )
}

pub fn check(deny: bool) -> Result<(), &'static str> {
    let case = match deny {
        true => "app-build-deny",
        false => "app-build",
    };
    wait_for(&format!("case/{}.done", Step::Image.name()))
        .map_err(|_| "app-build: pkg did not finish its image step")?;
    // The marker is durable once written, but `pkg` syncs it next, and an
    // action of the kernel's begun first would make that sync Busy.
    threads::sleep_ms(SYNC_MS);
    threads::spawn("terminal", terminal::run, 0)?;
    until(case, "desktop to read its ask box", STEP_MS, || {
        claim::showing() && ask_box::alive()
    })?;
    until(case, "the shell", STEP_MS, || {
        ipc::server_of("shell").is_some()
    })?;
    build(case)?;
    match deny {
        true => denied(case),
        false => allowed(case),
    }
}

/// The line, the build in the background, and its install card on
/// `desktop`, opened by itself; nothing is installed yet.
fn build(case: &str) -> Result<(), &'static str> {
    let started = line(case, LINE)?;
    if !started.contains(&format!("Building {NAME}:")) {
        log!("{case}: {LINE:?}: {started:?}");
        return Err("app-build: the line did not start building shopping");
    }
    log!("{case}: the line started building {NAME} in the background");
    until(
        case,
        "the terminal to take the install card",
        BUILD_MS,
        || terminal::cards_taken().0 == 1,
    )?;
    until(case, "the install card on desktop", STEP_MS, ask_box::shown)?;
    let question = ask_box::question().unwrap_or_default();
    if !question.contains(&format!("app_install(\"{NAME}\", \"0.0.1\", ")) {
        log!("{case}: the card asks {question:?}");
        return Err("app-build: the card is not shopping 0.0.1's install card");
    }
    if generation()? != 1 {
        return Err("app-build: a package was installed before the card was answered");
    }
    log!(
        "{case}: {NAME} 0.0.1 was built and tried, and its install card opened by itself on desktop; nothing is installed"
    );
    Ok(())
}

/// `y` on the card: `pkg` installs the app and starts it, and `undo` takes
/// it away while its source stays.
fn allowed(case: &str) -> Result<(), &'static str> {
    threads::sleep_ms(KEYS_MS);
    // "y" on the card: the person's Allow.
    log!("{case}: ready for keys");
    until(case, "the card's answer", STEP_MS, || !ask_box::shown())?;
    until(case, "the app to run", STEP_MS, || {
        !process::named(NAME).is_empty()
    })?;
    // The host waits for its window's "shown at" and "opened at" lines, so
    // `undo` must not take the app away before it paints: under TCG comphost
    // takes seconds to load the app after its process starts.
    until(case, "the app to serve", STEP_MS, || {
        ipc::server_of(NAME).is_some()
    })?;
    threads::sleep_ms(OPEN_MS);
    let installed = generation()?;
    if installed != 2 {
        log!("{case}: packages are generation {installed}");
        return Err("app-build: the Allow did not install generation 2");
    }
    log!("{case}: the Allow installed {NAME} 0.0.1 from local as generation 2, and it runs");
    let undone = line(case, "undo")?;
    if !undone.contains("packages back to generation 1") {
        log!("{case}: undo: {undone:?}");
        return Err("app-build: undo did not switch back to generation 1");
    }
    until(case, "the app to end", STEP_MS, || {
        process::named(NAME).is_empty()
    })?;
    source_stays(case)?;
    log!("{case}: undo removed {NAME}, and its source stays in /home/user/apps/{NAME}/");
    Ok(())
}

/// `n` on the card: nothing starts, and packages stay at generation 1.
fn denied(case: &str) -> Result<(), &'static str> {
    threads::sleep_ms(KEYS_MS);
    // "n" on the card: the person's Deny.
    log!("{case}: ready for keys");
    until(case, "the card's answer", STEP_MS, || !ask_box::shown())?;
    for _ in 0..QUIET_MS / POLL_MS {
        if !process::named(NAME).is_empty() {
            return Err("app-build: the app runs after a Deny");
        }
        threads::sleep_ms(POLL_MS);
    }
    let current = generation()?;
    if current != 1 {
        log!("{case}: packages are generation {current}");
        return Err("app-build: the Deny installed a package");
    }
    source_stays(case)?;
    log!(
        "{case}: the Deny installed nothing: packages are still generation 1, and {NAME}'s source stays in /home/user/apps/{NAME}/"
    );
    Ok(())
}

/// Waits up to `ms` for `done`.
fn until(
    case: &str,
    what: &'static str,
    ms: u64,
    mut done: impl FnMut() -> bool,
) -> Result<(), &'static str> {
    for _ in 0..ms / POLL_MS {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(POLL_MS);
    }
    log!("{case}: timed out waiting for {what}");
    Err(what)
}

/// `text` sent to the shell as the terminal sends it: the reply.
fn line(case: &str, text: &str) -> Result<String, &'static str> {
    let reply = ipc::call("shell", text.as_bytes(), BUILD_MS).map_err(|error| {
        log!("{case}: no reply to {text:?}: {error:?}");
        "app-build: a line got no reply"
    })?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// `/home/user/apps/shopping/main.c`, read as `fs`'s kernel client, still
/// holds what the model wrote.
fn source_stays(case: &str) -> Result<(), &'static str> {
    let open = Request::Open {
        path: format!("apps/{NAME}/main.c"),
        rights: Rights::READ.bits(),
        how: How::Existing,
    };
    let Reply::Opened(_, file) = ask(kabi::fs::HOME_USER, open)? else {
        return Err("app-build: the app's source is gone");
    };
    let source = match ask(file, Request::Read { off: 0, len: 4096 })? {
        Reply::Data(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        _ => return Err("app-build: the app's source cannot be read"),
    };
    if !source.contains(SOURCE_MARK) {
        log!("{case}: main.c holds {source:?}");
        return Err("app-build: the app's source is not what the model wrote");
    }
    Ok(())
}
