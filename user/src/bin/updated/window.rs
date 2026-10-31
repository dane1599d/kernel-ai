//! The restart window (ADR 0008 question 3, P4 plan question 10): once a
//! release is staged, `updated` restarts into it by itself only inside the
//! window the shell saves from a Settings click, `kai-updates 1` text in
//! `/system/state/restart/window`, which its plan line's read-only
//! `dir=restart` gives it, after a heads-up. With no window, the default, or a
//! file out of shape, the staged slot waits for the user's "restart"; its
//! one-shot is already set, so any restart boots it. Times are UTC.

use crate::folders::{self, Folders};
use crate::{idle, record, slot_of};
use alloc::format;
use alloc::string::String;
use bootslot::window::{DAY_MINUTES, FILE, HEADER, allows, minute_of_day, restart_window, window};
use bootslot::{Slot, Trial};
use kabi::handle::Handle;
use user::{println, sys};

/// How often the window is looked at again.
const CHECK_MS: u64 = 30_000;
/// The heads-up before a restart in the window.
const HEADS_UP_S: u64 = 60;
/// The test window `window=now` gives: from this minute, this long.
const TEST_MINUTES: u16 = 15;

/// What `disk::update` staged.
#[derive(Clone, Copy, Debug)]
pub struct Staged {
    pub slot: Slot,
    pub release_seq: u32,
}

/// Waits for the window, then restarts into `staged`; never returns. `args`
/// may set `heads_up_s=N`, and `window=now` (the boot case's) stands in for
/// the file with a window from the current minute.
pub fn wait(handle: Handle, staged: Staged, args: &str) -> ! {
    let word = |key: &str| args.split_whitespace().find_map(|w| w.strip_prefix(key));
    let heads_up = word("heads_up_s=")
        .and_then(|n| n.parse().ok())
        .unwrap_or(HEADS_UP_S);
    let test_now = word("window=") == Some("now");
    let saved = Folders::from_startup().ok().and_then(|f| f.restart);
    match saved.map(|d| folders::read(d, FILE)) {
        None => println!("updated: no /system/state/restart here"),
        Some(Ok(Some(b))) => {
            let text = core::str::from_utf8(&b).unwrap_or("");
            match restart_window(text).is_none() && text.trim_end() != HEADER {
                true => println!("updated: the saved restart window is out of shape; none is used"),
                false => println!("updated: the restart window's {FILE} is {} bytes", b.len()),
            }
        }
        Some(Ok(None)) => println!("updated: no restart window is saved"),
        Some(Err(why)) => println!("updated: the restart window's {FILE}: {why}"),
    }
    let (release, slot) = (staged.release_seq, staged.slot);
    let mut said = None;
    loop {
        let Ok(now) = sys::trust_time(handle).map(minute_of_day) else {
            println!("updated: FAIL TRUST_TIME; release {release} waits for \"restart\"");
            idle();
        };
        let w = match test_now {
            true => window(now, (now + TEST_MINUTES) % DAY_MINUTES),
            false => saved.and_then(read),
        };
        if said != Some(w) {
            match w {
                Some(w) => println!(
                    "updated: release {release} is staged in {slot:?}; the restart window is {} UTC",
                    span(w)
                ),
                None => println!(
                    "updated: release {release} is staged in {slot:?}; with no restart window it waits for \"restart\""
                ),
            }
            said = Some(w);
        }
        if w.is_some_and(|w| allows(w, now)) {
            println!(
                "updated: restarting into {slot:?} in {heads_up} s, inside the restart window"
            );
            sys::sleep_ms(heads_up * 1000);
            restart(handle, staged);
        }
        sys::sleep_ms(CHECK_MS);
    }
}

/// The window the saved text holds, if it holds one; text out of shape is
/// none, so an update never fails over it.
fn read(folder: Handle) -> Option<(u16, u16)> {
    let bytes = folders::read(folder, FILE).ok()??;
    restart_window(core::str::from_utf8(&bytes).ok()?)
}

/// Restarts into the staged slot, if it is still the one waiting.
fn restart(handle: Handle, staged: Staged) -> ! {
    let still = record(handle).and_then(|r| {
        let running = slot_of(
            sys::slot_info(handle, 1)
                .map_err(|e| format!("SLOT_INFO: {e:?}"))?
                .active,
        )?;
        let waiting = bootslot::trial(&r, staged.slot, staged.release_seq, running);
        Ok((r, waiting == Trial::Waiting))
    });
    match still {
        Ok((r, true)) => {
            println!(
                "updated: restarting into {:?} in the restart window",
                staged.slot
            );
            let set = sys::bootrec_set(handle, &r.encode(), staged.slot as u64, true);
            println!("updated: FAIL BOOTREC_SET: {set:?}");
        }
        Ok((_, false)) => println!("updated: the staged release changed; no restart"),
        Err(why) => println!("updated: FAIL {why}"),
    }
    idle()
}

/// `hh:mm-hh:mm`.
fn span((start, end): (u16, u16)) -> String {
    let clock = |m: u16| format!("{:02}:{:02}", m / 60, m % 60);
    format!("{}-{}", clock(start), clock(end))
}
