//! The held sends (`shellcore::outbox`) in the OS loop: a unit of their own
//! after the panel's line, so an `undo` it ran wins. Each one due starts (its
//! `sending` on disk first), the panel is told it is going, and only then is
//! it sent. What each came to, and the sends still held, go to the panel.

use crate::{System, desktop, report_to_desktop, start_of_unit};
use alloc::vec::Vec;
use shellcore::Shell;
use shellcore::outbox::Stage;
use user::{println, sys};

/// What this pass's take of the panel's requests came to (`take_panel`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// Nothing waited, or there is no desktop.
    Nothing,
    /// A line or a Stop, or a Terminal line taken before a due send: an undo
    /// may be queued behind it.
    Request,
    /// The desktop did not answer, or its answer did not read: an undo may
    /// wait there, so no send starts, and one left a minute past due ends
    /// unsent (`Shell::end_unchecked_sends`).
    Unanswered,
}

impl Taken {
    /// Whether a send must wait for the next pass.
    pub fn any(self) -> bool {
        self != Taken::Nothing
    }
}

/// The held sends last reported to the panel, as (entry, going), and the
/// desktop's pid then.
pub type SendsShown = Option<(u64, Vec<(u64, bool)>)>;

pub fn now_ms() -> u64 {
    sys::time_ns() / 1_000_000
}

/// One send a pass at most, and none on a pass that took a request (`took`):
/// the next pass takes the panel's next request first, so an undo queued
/// behind a line or a Stop, or clicked during a send's call, wins over the
/// next send (the wait is 1 ms while one is due). `shown` is the list last
/// reported, without its countdowns, and the desktop it went to.
pub fn give(shell: &mut Shell, system: &System, shown: &mut SendsShown, took: Taken) {
    start_of_unit();
    // A desktop that restarted has no list, and lost any undo clicked for
    // a held send: every held send ends unsent, and the list is told again.
    let pid = system.desktop_pid.get();
    if shown
        .as_ref()
        .is_some_and(|(was, _)| *was != pid && *was != 0)
    {
        shell.end_held_sends_on_restart();
        *shown = None;
    }
    if took == Taken::Unanswered {
        shell.end_unchecked_sends(now_ms());
    }
    if let Some(entry) = shell.start_send_after(took.any(), now_ms()) {
        report(shell, system, shown);
        shell.run_send(entry);
    }
    let ended = shell.take_sends_ended();
    let Some(route) = desktop() else {
        return;
    };
    for send in ended {
        let failed = !matches!(send.stage, Stage::Sent | Stage::Cancelled);
        let request = deskcore::sends::encode_sent(send.entry, failed, &send.line);
        if let Err(why) = report_to_desktop(route, &request) {
            println!("the panel was not told what a send came to: {why}");
        }
    }
    report(shell, system, shown);
}

/// The held sends to the panel, when they changed since `shown`.
fn report(shell: &Shell, system: &System, shown: &mut SendsShown) {
    let held = shell.held_sends(now_ms());
    let key: Vec<(u64, bool)> = held.iter().map(|s| (s.entry, s.sending)).collect();
    let pid = system.desktop_pid.get();
    if shown.as_ref() == Some(&(pid, key.clone())) {
        return;
    }
    let Some(route) = desktop() else {
        return;
    };
    let wire: Vec<deskcore::sends::WireSend> = held
        .into_iter()
        .map(|s| deskcore::sends::WireSend {
            entry: s.entry,
            sending: s.sending,
            left_ms: s.left_ms,
            host: s.host,
        })
        .collect();
    match report_to_desktop(route, &deskcore::sends::encode_sends(&wire)) {
        Ok(answer) if answer == "ok" => *shown = Some((pid, key)),
        Ok(answer) => println!("the panel did not take the held sends: {answer}"),
        Err(why) => println!("the panel did not take the held sends: {why}"),
    }
}
