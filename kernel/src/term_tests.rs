//! `test=desk-term` (P3 task 8): the kernel's terminal in a `desktop` window.
//! `term` mirrors the console in its "Terminal" window; a click on its title bar
//! gives it the focus, and `desktop` lends the keys to the kernel's terminal.
//! The harness types a line the mock model answers with a change it was not
//! asked for, then `y`: the console must take the screen for the Allow / Deny
//! menu and end the lease, the `y` must not answer, and the `n` typed once the
//! console shows must. Then a click on the panel moves the focus away, and a
//! line typed after reaches `desktop`'s queue, not the terminal.

use crate::desk_tests::report;
use crate::display::claim;
use crate::ui_tests::DESK;
use crate::{console, input, log, process, terminal, threads};

/// The plan lines this case adds: `desktop` as `desk-boot` runs it (its fonts
/// need the stack), with the burst `llm` has, since painting the Terminal's
/// first frame under TCG ran past the default 2 s, and `term`.
pub const PLAN: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512 burst_ms=10000\n\
     term uses=desktop caps=call+term pages=4096 stack_pages=512";
const WAIT_MS: u64 = 60_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = DESK.screen_centre();
/// A point on the Terminal window's title bar: it is the first window.
const TITLE: (i32, i32) = DESK.window_title(0, 102);
/// A point on the empty desk: a click there takes the focus from the
/// Terminal's window, which ends its key lease (Ctrl+Space would go to the
/// console under the lease).
const DESK_POINT: (i32, i32) = DESK.desk_point();
/// Time for the harness's keys, as `ask_tests` gives them.
const KEYS_MS: u64 = 4_000;
/// Time for the last batch, a whole line and Enter typed into the pane:
/// xtask types a key every 150 ms after 200 ms to reach the monitor, so the
/// 29 keys of "what is the scheduling policy" and Enter take about 4.6 s.
/// The case must not end before the panel queues the line.
const LINE_MS: u64 = 9_000;

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    log!("desk-term: timed out waiting for {what}");
    Err(what)
}

fn showing() -> Option<u64> {
    let &(pid, _) = process::named("desktop").first()?;
    let slot = process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))?;
    claim::shows(slot).then_some(pid)
}

/// Clicks at `to`, moving there from `at`.
fn click(at: &mut (i32, i32), to: (i32, i32)) {
    report(to.0 - at.0, to.1 - at.1, 0);
    *at = to;
    report(0, 0, 1);
    report(0, 0, 0);
}

pub fn check() -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    wait("desktop and term", || {
        showing().is_some() && !process::named("term").is_empty()
    })?;
    // term opens its window with its first call.
    threads::sleep_ms(3_000);
    let mut at = START;
    click(&mut at, TITLE);
    wait("desktop to lend the keys to the terminal", input::leased)?;
    log!("desk-term: ready for keys");
    wait("the console to take the screen for the ask", || {
        terminal::awaiting_answer() && showing().is_none() && !console::suspended()
    })?;
    if input::leased() {
        return Err("desk-term: the lease outlived the ask's takeover");
    }
    // The `y` typed after the line, as fast as it came, must not answer.
    threads::sleep_ms(KEYS_MS);
    if !terminal::awaiting_answer() {
        return Err("desk-term: a key typed for the window answered the ask");
    }
    log!("desk-term: ready for keys");
    wait("the answer typed on the console", || {
        !terminal::awaiting_answer()
    })?;
    wait("desktop to get the screen back", || showing().is_some())?;
    log!("desk-term: an ask typed in the terminal window was answered only on the console");
    // The new desk, and term's window on it.
    threads::sleep_ms(3_000);
    at = START;
    click(&mut at, DESK_POINT);
    threads::sleep_ms(1_000);
    if input::leased() {
        return Err("desk-term: the keys stayed with the terminal after the focus moved");
    }
    // The lease is over, so Ctrl+Space reaches desktop: the pane's input
    // takes the last line, which the panel queues for the shell.
    crate::ui_tests::focus_kai();
    threads::sleep_ms(1_000);
    let (keys, asked) = (input::keys(), terminal::replies_shown());
    log!("desk-term: ready for keys");
    threads::sleep_ms(LINE_MS);
    if input::keys() == keys || terminal::replies_shown() != asked {
        return Err("desk-term: keys typed with the focus elsewhere reached the terminal");
    }
    log!("desk-term: with the focus in the pane the keys went to desktop, and nothing asked");
    Ok(())
}
