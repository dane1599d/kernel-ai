//! The restart window (ADR 0008 question 3, P4 plan question 10): once a
//! release is staged, `updated` restarts into it by itself only inside the
//! window the Settings app keeps in `/system/settings/updates.cbor`, which its
//! plan line's read-only `dir=settings` gives it, after a heads-up. With no
//! window, the default, the staged slot waits for the user's "restart"; its
//! one-shot is already set, so any restart boots it. Times are UTC.

use crate::folders::{self, Folders};
use crate::{idle, record, slot_of};
use alloc::format;
use alloc::string::String;
use bootslot::window::{DAY_MINUTES, allows, minute_of_day, restart_window, window};
use bootslot::{Slot, Trial};
use kabi::handle::Handle;
use user::{println, sys};

/// The Settings app's file for updates.
const FILE: &str = "updates.cbor";
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
    let settings = Folders::from_startup().ok().and_then(|f| f.settings);
    match settings.map(|d| folders::read(d, FILE)) {
        None => println!("updated: no /system/settings here"),
        Some(Ok(Some(b))) => println!("updated: /system/settings/{FILE} is {} bytes", b.len()),
        Some(Ok(None)) => println!("updated: /system/settings has no {FILE}"),
        Some(Err(why)) => println!("updated: /system/settings/{FILE}: {why}"),
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
            false => settings.and_then(read),
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

/// The window in `/system/settings/updates.cbor`, if it holds one.
fn read(settings: Handle) -> Option<(u16, u16)> {
    let bytes = folders::read(settings, FILE).ok()??;
    restart_window(&bytes)
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
