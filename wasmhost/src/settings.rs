//! `kai:settings@0.1.0` for `lifecycle 2` (P4 task 7): a granted interface
//! through which the Settings app reads and changes the system's settings. As
//! with `kai:edit`, `send` only queues the request; comphost takes the queue
//! after the call ([`CoreInstance::take_settings`]), passes each request on and
//! gives the app the answer in a later call, `replies.on-reply`
//! ([`CoreInstance::settings_reply`]). So a call never waits on the system, and
//! a version rehearsed beside the active one cannot send a request twice while
//! it runs: only the shown version's queue is taken.
//!
//! Each request is checked when it is sent ([`classify`]) against the whole
//! grammar of its verb, not only the verb, and tagged [`Kind::Read`] or
//! [`Kind::Change`]. A read is one of eight exact lines. A change is passed on
//! only for a real key or click: the request carries `user`, true only when it
//! was queued during [`CoreInstance::ui_input`] or
//! [`CoreInstance::ui_events_user`], and a call may send one change. A call
//! that faults leaves nothing queued. Everything else, including every call the
//! agent makes through `ui_act`, has `user` false, so comphost refuses it.
//!
//! The interface is linked only for an instance loaded with the `settings`
//! grant ([`Grants`]); without it the import fails to link, and the version
//! does not stage (`Reason::Link`).

use crate::canonical::CoreInstance;
use crate::grants::Grants;
use crate::instance::HostState;
use crate::tables;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use appabi::Val;
use livecore::{Fault, Reason};
use wasmi::{Caller, Error, Extern};

/// The interface whose function this module serves.
pub(crate) const SETTINGS: &str = "kai:settings/settings";
/// The export the answers go to.
pub(crate) const REPLIES: &str = "kai:settings/replies";

/// Longest request `send` takes, in bytes.
pub const MAX_REQUEST: usize = 512;
/// Most requests waiting for comphost; a call that sends more traps.
pub const MAX_QUEUED: usize = 16;
/// Longest name sent (a profile, an app): `kabi::MAX_NAME`.
pub const MAX_NAME: usize = 24;
/// Longest effect row `grants-revoke` and `grants-restore` name, as an app's
/// declared action (`deskcore`'s `MAX_ACTION`).
pub const MAX_ROW: usize = 64;
/// Longest value of `appearance-set`.
pub const MAX_VALUE: usize = 64;
/// Largest answer the app is given; a larger one is replaced by an error.
pub const MAX_REPLY: usize = 64 << 10;
/// Longest error text the app is given.
pub const MAX_ERROR: usize = 200;
/// Minutes in a day: the restart window's ends are below this.
pub const DAY_MINUTES: u32 = 24 * 60;

/// The reads, each the whole request.
pub const READS: &[&str] = &[
    "rules",
    "grants",
    "about",
    "profiles",
    "apps",
    "file appearance",
    "file apps",
    "file updates",
];
/// The settings `appearance-set` takes (`look::appearance`).
pub const APPEARANCE_SETTINGS: &[&str] = &[
    "theme",
    "scale",
    "text_size",
    "token",
    "reduced_motion",
    "legible",
];
/// The verbs of the changes; each has its own arguments ([`classify`]).
pub const CHANGES: &[&str] = &[
    "appearance-set",
    "profile-set",
    "rules-remove",
    "grants-revoke",
    "grants-restore",
    "keep-running",
    "restart-window",
];

/// What a request does to the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Reads; may be passed on at any time.
    Read,
    /// Changes something; only for a real key or click.
    Change,
}

/// A request the app sent, waiting for comphost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsRequest {
    /// What `on-reply` is told the answer is for.
    pub ticket: u32,
    pub request: String,
    pub kind: Kind,
    /// Whether it was queued while handling a real key or click. A change with
    /// this false is never passed on.
    pub user: bool,
}

/// The requests not yet taken, and the next ticket.
#[derive(Default)]
pub(crate) struct SettingsHost {
    pub(crate) queue: Vec<SettingsRequest>,
    next: u32,
    /// Where this call's requests start in the queue, and the changes it sent.
    pub(crate) call_start: usize,
    pub(crate) changes_in_call: u32,
}

/// Printable ASCII, the only characters a request holds.
fn printable(text: &str) -> bool {
    text.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// A name: 1 to `max` printable ASCII bytes, with no space at either end and no
/// two spaces together.
pub fn name_ok(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.len() <= max
        && printable(text)
        && !text.starts_with(' ')
        && !text.ends_with(' ')
        && !text.contains("  ")
}

/// A decimal number of 1 to `digits` ASCII digits.
fn number(text: &str, digits: usize) -> Option<u32> {
    let ok = !text.is_empty() && text.len() <= digits && text.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| text.parse().ok()).flatten()
}

/// A rule: `3`, or `S3` for a standing instruction.
fn rule(text: &str) -> bool {
    number(text.strip_prefix('S').unwrap_or(text), 4).is_some()
}

/// The restart window: two minutes of the day that differ, or `off`.
fn window(text: &str) -> bool {
    if text == "off" {
        return true;
    }
    let Some((start, end)) = text.split_once(' ') else {
        return false;
    };
    match (number(start, 4), number(end, 4)) {
        (Some(s), Some(e)) => s < DAY_MINUTES && e < DAY_MINUTES && s != e,
        _ => false,
    }
}

/// An app as the shell names it in `grants`: 1 to [`MAX_NAME`] bytes of
/// lowercase letters, digits, `-` and `_`, so a request splits on spaces alone.
fn app_ok(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_NAME
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// An effect row an app declares (`notes_add`): 1 to [`MAX_ROW`] bytes of
/// lowercase letters, digits and `_`, neither starting nor ending with `_`.
fn row_ok(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_ROW
        && !text.starts_with('_')
        && !text.ends_with('_')
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// `<app> <row>`, as `grants-revoke` and `grants-restore` take them.
fn app_row(args: &str) -> bool {
    args.split_once(' ')
        .is_some_and(|(app, row)| app_ok(app) && row_ok(row))
}

/// What `request` is, or why it is not a request an app may send. A read is one
/// of [`READS`], exactly, or `grants <app>`. A change is a verb of [`CHANGES`] with arguments that
/// fit that verb's grammar. Only printable ASCII is allowed, so no control,
/// bidirectional or look-alike character reaches a log or a handler.
pub fn classify(request: &str) -> Result<Kind, String> {
    if request.is_empty() || request.len() > MAX_REQUEST {
        return Err(alloc::format!(
            "{} bytes; 1 to {MAX_REQUEST}",
            request.len()
        ));
    }
    if !printable(request) {
        return Err(String::from("only printable ASCII"));
    }
    if READS.contains(&request) || request.strip_prefix("grants ").is_some_and(app_ok) {
        return Ok(Kind::Read);
    }
    let (verb, args) = match request.split_once(' ') {
        Some((verb, args)) => (verb, args),
        None => (request, ""),
    };
    let fits = match verb {
        "appearance-set" => args.split_once(' ').is_some_and(|(setting, value)| {
            APPEARANCE_SETTINGS.contains(&setting) && name_ok(value, MAX_VALUE)
        }),
        "profile-set" => name_ok(args, MAX_NAME),
        "rules-remove" => rule(args),
        "grants-revoke" | "grants-restore" => app_row(args),
        "keep-running" => args
            .split_once(' ')
            .is_some_and(|(flag, app)| matches!(flag, "on" | "off") && name_ok(app, MAX_NAME)),
        "restart-window" => window(args),
        _ => return Err(alloc::format!("`{verb}` is not a request")),
    };
    if fits {
        Ok(Kind::Change)
    } else {
        Err(alloc::format!("`{verb}` does not take those arguments"))
    }
}

/// `settings.send(request) -> u32`: the request's ticket.
pub(crate) fn send(mut caller: Caller<'_, HostState>, ptr: i32, len: i32) -> Result<i32, Error> {
    let len = len as u32 as usize;
    if len > MAX_REQUEST {
        return Err(Error::new(alloc::format!(
            "{SETTINGS} send: {len} bytes; 1 to {MAX_REQUEST}"
        )));
    }
    let start = ptr as u32 as usize;
    let bytes = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .and_then(|memory| {
            memory
                .data(&caller)
                .get(start..start.saturating_add(len))
                .map(<[u8]>::to_vec)
        })
        .ok_or_else(|| Error::new("bytes outside memory"))?;
    let request = String::from_utf8(bytes)
        .map_err(|_| Error::new(alloc::format!("{SETTINGS} send: not UTF-8 text")))?;
    let kind =
        classify(&request).map_err(|why| Error::new(alloc::format!("{SETTINGS} send: {why}")))?;
    // Whether a person's key or click is being handled (`HostState::user_input`).
    let user = caller.data().user_input;
    let host = &mut caller.data_mut().settings;
    if host.queue.len() >= MAX_QUEUED {
        return Err(Error::new(alloc::format!(
            "{SETTINGS} send: {MAX_QUEUED} requests waiting for comphost"
        )));
    }
    if kind == Kind::Change {
        // One real key or click authorises one change.
        if host.changes_in_call > 0 {
            return Err(Error::new(alloc::format!(
                "{SETTINGS} send: a call may send one change"
            )));
        }
        host.changes_in_call += 1;
    }
    let ticket = host.next;
    host.next = host
        .next
        .checked_add(1)
        .ok_or_else(|| Error::new(alloc::format!("{SETTINGS} send: tickets used up")))?;
    host.queue.push(SettingsRequest {
        ticket,
        request,
        kind,
        user,
    });
    Ok(ticket as i32)
}

/// Whether `grants` lets an app import `kai:settings/settings`.
pub(crate) fn granted(grants: Grants) -> bool {
    grants.settings
}

/// An error text the app is given: printable ASCII, and cut.
fn error_text(why: &str) -> String {
    why.chars()
        .map(|c| {
            if c.is_ascii() && printable(c.encode_utf8(&mut [0; 4])) {
                c
            } else {
                '?'
            }
        })
        .take(MAX_ERROR)
        .collect()
}

impl CoreInstance {
    /// Starts counting a call's requests: where they start in the queue, and
    /// that it has sent no change yet.
    pub(crate) fn settings_call_begins(&mut self) {
        let host = &mut self.store.data_mut().settings;
        host.call_start = host.queue.len();
        host.changes_in_call = 0;
    }

    /// A call faulted: what it queued is not sent.
    pub(crate) fn settings_call_faulted(&mut self) {
        let host = &mut self.store.data_mut().settings;
        let start = host.call_start.min(host.queue.len());
        host.queue.truncate(start);
    }

    /// The requests the app sent since the last call here, oldest first.
    pub fn take_settings(&mut self) -> Vec<SettingsRequest> {
        core::mem::take(&mut self.store.data_mut().settings.queue)
    }

    /// Tells the app the answer to `ticket`, or why there is none. An answer
    /// over [`MAX_REPLY`] is replaced by an error, and an error text is cut and
    /// made printable. An app that does not export `kai:settings/replies` is
    /// told nothing.
    pub fn settings_reply(
        &mut self,
        ticket: u32,
        outcome: Result<Vec<u8>, String>,
    ) -> Result<(), Fault> {
        let Some(export) = self.on_settings_reply.clone() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == REPLIES && f.name == "on-reply")
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:settings/replies table"))?;
        let text = |s: String| Some(Box::new(Val::String(s)));
        let outcome = match outcome {
            Ok(bytes) if bytes.len() <= MAX_REPLY => {
                Val::Result(Ok(Some(Box::new(Val::Bytes(bytes)))))
            }
            Ok(_) => Val::Result(Err(text(String::from("the answer is too large")))),
            Err(why) => Val::Result(Err(text(error_text(&why)))),
        };
        livecore::Instance::call(self, &entry.func(export), &[Val::U32(ticket), outcome])?;
        Ok(())
    }
}
