//! What a script changed, recorded as it happens with the value it replaced, so
//! `undo` can put it back. A change with no recorded inverse cannot be run-tier.

use crate::tune::spec_of;
use crate::{Host, Proof};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::Write;
use kabi::policy;

/// How long to wait before trying a refused switch once more: the kernel takes a
/// policy switch at most every 100 ms.
const BUSY_RETRY_MS: u64 = 100;

/// How to put one change back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inverse {
    /// Install `policy` and `preset` again, but only while `after` (the policy and
    /// preset the change installed) is still what runs.
    Spec {
        policy: u64,
        preset: u64,
        after: (u64, u64),
    },
    /// Stop the service a run started again.
    Started(Name),
    /// Start the service an undo stopped again: the redo of [`Inverse::Started`].
    Stopped(Name),
    /// Make package generation `to` current again, but only while `from` (the one
    /// the switch made) still is. The shell's package store does it
    /// (`Shell::revert_entry`), not the host.
    Generation { from: u64, to: u64 },
    /// Make generation `to` of `pkg`'s store current again while `from`, the
    /// one a `cc build` install made, still is: `pkg`'s `switch-back`, which
    /// goes back to the parent only, so an install is not redone (P4 task 13).
    Local { from: u64, to: u64 },
    /// Restore what `fs` transaction `txn` replaced (P2 task 11); the `fs`
    /// server refuses one over a later change.
    Fs { txn: u64 },
    /// Put back what standing instruction `rule`'s firing (`action` on `app`)
    /// did, through its owner; with `undo` false, do it again (P3 task 11).
    Fired {
        rule: u64,
        action: Name,
        app: Name,
        undo: bool,
    },
    /// Put back every process's class and quota from before a profile switch,
    /// kept by the shell under `key` (P3 task 12; `Shell::revert_entry`).
    Profile { key: u64 },
    /// Remove standing instruction `id` again (`made`: `undo` of `rules_add`),
    /// or put a removed one back (`undo` of `rules_remove`). The shell's store
    /// does it (`Shell::revert_entry`), not the host.
    Instruction { id: u64, made: bool },
    /// Keep app `app` running (`on`) or let it nap again, as it was before a
    /// Settings toggle. The shell's list does it (`Shell::revert_keep`).
    Keep { app: Name, on: bool },
    /// Pause goal `id` again (`paused`) or resume it, as it was before a
    /// `goal_resume` or `goal_pause` (G2). The shell queues it for `goals`
    /// (`Shell::revert_goal`).
    Goal { id: u64, paused: bool },
    /// Put app `app`'s state back from before its call kept as `id` (ADR 0003
    /// §4's checkpoint undo, `kai-undo 1`): the app runs its own inverse.
    App { app: Name, id: u64 },
    /// Set appearance `setting` back to `before`, while it is still `after`
    /// (P4 task 8's `appearance_set`).
    Appearance {
        setting: Name,
        before: Name,
        after: Name,
    },
    /// Put keyboard focus back on `to` (`w2`, `panel` or `none`); its redo
    /// focuses `from` again (P3 task 10's `ui_focus`).
    Refocus { to: Name, from: Name },
    /// Open the app whose window `window_close` closed.
    Reopen(Name),
    /// Close `window`, which `ui_open` opened for `app`. It asks first, since
    /// the window may hold work by now.
    Reclose { window: Name, app: Name },
    /// Use model `back` from the next restart again, while the next restart's
    /// model is still `from` (P4 task 10's `model_use`); none when no model
    /// was in use, which cannot be gone back to. The shell asks `updated`
    /// (`Shell::revert_entry`).
    Model {
        back: Option<[u8; 32]>,
        from: [u8; 32],
    },
    /// Remove model blob `sha` from the store, which `model_fetch` brought in,
    /// or stop its download and drop the part downloaded. `updated` refuses
    /// while that blob runs or is on trial. Its redo is none: fetching again
    /// brings gigabytes, so it is a fresh `model_fetch`, which asks
    /// (`Shell::revert_entry`).
    Fetched { sha: [u8; 32] },
    /// Withhold `row` from `app`'s effects again (`withheld`), or give it back:
    /// the undo of `grant_restore` and of `grant_revoke` (P4 task 7).
    Grant {
        app: Name,
        row: Name,
        withheld: bool,
    },
    /// Take network grant `id` back (`held`: the undo of the Allow that made
    /// it), or put a revoked one back (the undo of a Settings revoke). The
    /// shell's store does it (`Shell::revert_net_grant`), not the host.
    NetGrant { id: u32, held: bool },
    /// Cancel the send log entry `entry` holds in the outbox, while it waits
    /// (`crate::outbox`). Never written to the log (it says `none` there), so
    /// a send read back from disk has no undo and never goes; its redo is
    /// none, since a send always asks again.
    Unsend { entry: u64 },
    /// An inverse this shell cannot read, as `/system/log` holds it: one a
    /// newer shell wrote, read back after a slot rollback. It has no undo
    /// here, and is written back word for word, so the newer shell finds it
    /// again once the slot moves forward.
    Unread(Raw),
}

impl Inverse {
    /// Whether this shell can put the change back: every inverse but one it
    /// could not read.
    pub fn is_read(&self) -> bool {
        !matches!(self, Inverse::Unread(_))
    }
}

/// The longest unreadable inverse kept word for word; a longer one is lost
/// (`none`), as before.
pub const MAX_RAW: usize = 160;

/// An inverse's word as the log held it, without the heap, so an inverse
/// stays `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raw {
    bytes: [u8; MAX_RAW],
    len: u8,
}

impl Raw {
    /// `None` for a word longer than [`MAX_RAW`], or one with a space or a
    /// control character, which a log line could not hold as one word.
    pub fn new(word: &str) -> Option<Self> {
        if word.is_empty() || word.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return None;
        }
        let mut bytes = [0u8; MAX_RAW];
        bytes
            .get_mut(..word.len())?
            .copy_from_slice(word.as_bytes());
        Some(Self {
            bytes,
            len: word.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("none")
    }
}

/// A service name held without the heap, so an inverse stays `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; kabi::MAX_NAME],
    len: u8,
}

impl Name {
    /// `None` for a name longer than a process name can be.
    pub fn new(name: &str) -> Option<Self> {
        let mut bytes = [0u8; kabi::MAX_NAME];
        bytes
            .get_mut(..name.len())?
            .copy_from_slice(name.as_bytes());
        Some(Self {
            bytes,
            len: name.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }

    /// The name as one part of an inverse's word: `%`, `:`, `=`, spaces and
    /// control characters as `%XX`, so a time zone's `+05:30`, or a name
    /// with a space, reads back whole (bug-hunt-3 H3-24).
    fn escaped(&self) -> String {
        let mut out = String::new();
        for c in self.as_str().chars() {
            if matches!(c, '%' | ':' | '=') || c.is_whitespace() || c.is_control() {
                let mut utf8 = [0u8; 4];
                for b in c.encode_utf8(&mut utf8).bytes() {
                    let _ = write!(out, "%{b:02X}");
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// [`Name::escaped`] read back. A `%` without two hex digits after it
    /// stays as it is, as a shell before the escape wrote it.
    fn from_word(word: &str) -> Option<Self> {
        let bytes = word.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while let Some(&b) = bytes.get(at) {
            let hex = bytes
                .get(at + 1..at + 3)
                .filter(|h| b == b'%' && h.iter().all(u8::is_ascii_hexdigit))
                .and_then(|h| u8::from_str_radix(core::str::from_utf8(h).ok()?, 16).ok());
            match hex {
                Some(byte) => {
                    out.push(byte);
                    at += 3;
                }
                None => {
                    out.push(b);
                    at += 1;
                }
            }
        }
        Self::new(core::str::from_utf8(&out).ok()?)
    }
}

/// Why a change was not put back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Something else changed it since, to this (in the user's words); putting the
    /// old value back would overwrite that, so the user is asked first.
    Moved(String),
    /// It could not be put back yet.
    Failed(String),
    /// It was not put back, and asking again would not change that: `fs`
    /// keeps what a later change touched.
    Kept(String),
}

/// One change a script made, as the action log records it.
pub type Done = crate::actlog::Change;

/// The changes of one run, in order.
pub type Journal = Rc<RefCell<Vec<Done>>>;

/// The policy and preset installed now, to record before changing them. `None`
/// for a policy outside the catalog, which cannot be installed again.
pub fn current_spec(host: &dyn Host) -> Option<(u64, u64)> {
    let (policy, preset) = match host.sched_stats(0) {
        Ok(stats) => (stats.policy, stats.preset),
        Err(_) => (host.metrics().ok()?.policy, kabi::sched::preset::GENTLE),
    };
    (policy != policy::OTHER).then_some((policy, preset))
}

/// `round-robin`, `classes/firm`: a policy as the user sees it.
pub fn spec_name(policy: u64, preset: u64) -> String {
    format!("{}", spec_of(policy, preset).label())
}

/// What `inverse` puts back, in the user's words: `interactive`.
pub fn target(inverse: Inverse) -> String {
    match inverse {
        Inverse::Spec { policy, preset, .. } => spec_name(policy, preset),
        Inverse::Started(name) => format!("{} stopped", name.as_str()),
        Inverse::Stopped(name) => format!("{} running", name.as_str()),
        Inverse::Generation { to, .. } | Inverse::Local { to, .. } => format!("generation {to}"),
        Inverse::Fs { .. } => String::from("the files as they were"),
        Inverse::Fired { app, .. } => format!("{} as it was", app.as_str()),
        Inverse::Instruction { id, made: true } => format!("standing instruction S{id} removed"),
        Inverse::Instruction { id, made: false } => format!("standing instruction S{id}"),
        Inverse::Profile { .. } => String::from("the profile before"),
        Inverse::Keep { app, .. } => format!("{} keep-running as before", app.as_str()),
        Inverse::Goal { id, paused: true } => format!("goal {id} paused again"),
        Inverse::Goal { id, paused: false } => format!("goal {id} going again"),
        Inverse::App { app, .. } => format!("{} as it was", app.as_str()),
        Inverse::Appearance {
            setting, before, ..
        } => {
            format!("{} {}", setting.as_str(), before.as_str())
        }
        Inverse::Refocus { to, .. } => format!("focus on {}", to.as_str()),
        Inverse::Reopen(app) => format!("{} open", app.as_str()),
        Inverse::Reclose { app, .. } => format!("{} closed", app.as_str()),
        Inverse::Grant { app, row, withheld } => grant_words(app, row, withheld),
        Inverse::Model { .. } => String::from("the model before, from the next restart"),
        Inverse::Fetched { .. } => String::from("the fetched model removed"),
        Inverse::NetGrant { id, held: true } => format!("network grant #{id} taken back"),
        Inverse::NetGrant { id, held: false } => format!("network grant #{id}"),
        Inverse::Unsend { .. } => String::from("the send not sent"),
        Inverse::Unread(_) => String::from("what a newer shell recorded"),
    }
}

/// `notes may not call fs_write`, `notes may call fs_write`.
fn grant_words(app: Name, row: Name, withheld: bool) -> String {
    let may = if withheld { "may not" } else { "may" };
    format!("{} {may} call {}", app.as_str(), row.as_str())
}

/// Puts one change back; what it did, or why it did not. Compare-and-set: a value
/// that moved since the change is left alone unless `anyway`. A switch the kernel
/// refuses as too soon is tried once more after [`BUSY_RETRY_MS`].
pub fn revert(
    host: &dyn Host,
    inverse: Inverse,
    anyway: bool,
    ids: Ids,
) -> Result<(String, Inverse), Refused> {
    match inverse {
        // A restore is a transaction of its own, so its redo restores that one.
        // One that left files a later change touched still landed: its redo
        // puts back what it did restore (bug-hunt-6 H6-22).
        Inverse::Fs { txn } => match host.fs_undo(txn) {
            Ok(crate::FsUndone { generation, left }) => Ok((
                match left {
                    0 => String::from("files back as they were"),
                    n => format!(
                        "files back as they were, but {n} changed since stay as they are now"
                    ),
                },
                Inverse::Fs { txn: generation },
            )),
            Err(crate::FsRefused::Failed(why)) => Err(Refused::Failed(why)),
            Err(crate::FsRefused::Kept(why)) => Err(Refused::Kept(why)),
        },
        // An app's undo is kept as a call of its own, so its redo undoes that.
        Inverse::App { app, id } => {
            let redo = crate::apps::undo(host, app.as_str(), id, anyway)?;
            Ok((
                format!("{} as it was", app.as_str()),
                Inverse::App { app, id: redo },
            ))
        }
        Inverse::Fired {
            rule,
            action,
            app,
            undo,
        } => {
            let triggers = host
                .triggers()
                .ok_or_else(|| Refused::Failed(String::from("no desktop events here yet")))?;
            triggers
                .run(rule, action.as_str(), app.as_str(), undo)
                .map_err(Refused::Failed)?;
            let text = match undo {
                true => format!("{} as it was", app.as_str()),
                false => format!("{}({}) again", action.as_str(), app.as_str()),
            };
            Ok((text, reverse(inverse)))
        }
        // Each knows its redo from what desktop answered.
        Inverse::Refocus { .. } | Inverse::Reopen(_) | Inverse::Reclose { .. } => {
            crate::ui::revert(host, inverse, anyway)
        }
        other => revert_one(host, other, anyway, ids).map(|text| (text, reverse(other))),
    }
}

fn revert_one(
    host: &dyn Host,
    inverse: Inverse,
    anyway: bool,
    ids: Ids,
) -> Result<String, Refused> {
    match inverse {
        Inverse::Spec {
            policy,
            preset,
            after,
        } => {
            let now = current_spec(host);
            if !anyway && now != Some(after) {
                let now = now.map_or_else(|| String::from("unknown"), |(p, s)| spec_name(p, s));
                return Err(Refused::Moved(now));
            }
            let mut result = host.set_spec(policy, preset);
            if result == Err(kabi::Error::Busy) {
                host.sleep_ms(BUSY_RETRY_MS);
                result = host.set_spec(policy, preset);
            }
            result
                .map(|()| format!("policy back to {}", spec_name(policy, preset)))
                .map_err(|error| {
                    Refused::Failed(format!("cannot switch the policy back yet ({error:?})"))
                })
        }
        Inverse::Started(name) => host
            .stop_service(name.as_str(), Proof::Undo { id: ids.made })
            .map(|()| format!("service {} stopped again", name.as_str()))
            .map_err(Refused::Failed),
        Inverse::Stopped(name) => host
            .start_service(name.as_str(), ids.next)
            .map(|()| format!("service {} started again", name.as_str()))
            .map_err(Refused::Failed),
        Inverse::Local { from, to } if to < from => host
            .pkg(&format!("switch-back {to} {from}"))
            .map(|_| format!("packages back to generation {to}"))
            .map_err(Refused::Failed),
        Inverse::Local { .. } => Err(Refused::Failed(String::from(
            "a local build is installed again by cc build, or by Kai's install card",
        ))),
        Inverse::Generation { .. } => Err(Refused::Failed(String::from(
            "a package switch is put back by the shell's store",
        ))),
        Inverse::Fs { .. }
        | Inverse::Fired { .. }
        | Inverse::Refocus { .. }
        | Inverse::Reopen(_)
        | Inverse::Reclose { .. }
        | Inverse::App { .. } => Err(Refused::Failed(String::from("it is put back by revert"))),
        Inverse::Appearance {
            setting,
            before,
            after,
        } => crate::appearance::revert(host, setting, before, after, anyway),
        // Setting it twice changes nothing, so this needs no compare.
        Inverse::Grant { app, row, withheld } => {
            crate::grants::set(host, app.as_str(), row.as_str(), withheld)
                .map(|_| grant_words(app, row, withheld))
                .map_err(Refused::Failed)
        }
        Inverse::Instruction { .. }
        | Inverse::Profile { .. }
        | Inverse::Keep { .. }
        | Inverse::Goal { .. }
        | Inverse::NetGrant { .. }
        | Inverse::Unsend { .. }
        | Inverse::Model { .. }
        | Inverse::Fetched { .. } => Err(Refused::Failed(String::from(
            "it is put back by the shell's own store",
        ))),
        Inverse::Unread(_) => Err(Refused::Failed(String::from(
            "a newer shell recorded how, and this one cannot read it",
        ))),
    }
}

/// The log entries a revert stands between: `made`, whose change it puts back,
/// and `next`, the entry the revert itself is logged as.
#[derive(Clone, Copy, Debug)]
pub struct Ids {
    pub made: u64,
    pub next: u64,
}

/// How to put back what reverting with `inverse` did: the redo.
pub fn reverse(inverse: Inverse) -> Inverse {
    match inverse {
        Inverse::Spec {
            policy,
            preset,
            after,
        } => Inverse::Spec {
            policy: after.0,
            preset: after.1,
            after: (policy, preset),
        },
        Inverse::Started(name) => Inverse::Stopped(name),
        Inverse::Stopped(name) => Inverse::Started(name),
        Inverse::Generation { from, to } => Inverse::Generation { from: to, to: from },
        Inverse::Local { from, to } => Inverse::Local { from: to, to: from },
        // Only the `fs` server knows the restore's own transaction ([`revert`]),
        // and only the app its undo's id.
        Inverse::Fs { txn } => Inverse::Fs { txn },
        Inverse::App { app, id } => Inverse::App { app, id },
        Inverse::Fired {
            rule,
            action,
            app,
            undo,
        } => Inverse::Fired {
            rule,
            action,
            app,
            undo: !undo,
        },
        Inverse::Instruction { id, made } => Inverse::Instruction { id, made: !made },
        Inverse::Keep { app, on } => Inverse::Keep { app, on: !on },
        Inverse::Goal { id, paused } => Inverse::Goal {
            id,
            paused: !paused,
        },
        Inverse::NetGrant { id, held } => Inverse::NetGrant { id, held: !held },
        // Never used: the undo of a cancel is logged with no inverse
        // (`Shell::revert_entry`), so it never sends.
        Inverse::Unsend { entry } => Inverse::Unsend { entry },
        // Never used either: a removed model's redo is logged with no inverse.
        Inverse::Fetched { sha } => Inverse::Fetched { sha },
        Inverse::Unread(raw) => Inverse::Unread(raw),
        Inverse::Appearance {
            setting,
            before,
            after,
        } => Inverse::Appearance {
            setting,
            before: after,
            after: before,
        },
        // Only the shell knows the redo's key ([`crate::Shell`]).
        Inverse::Profile { key } => Inverse::Profile { key },
        Inverse::Refocus { to, from } => Inverse::Refocus { to: from, from: to },
        // `revert` names the window a reopen made; this is only its fallback.
        Inverse::Reopen(app) => Inverse::Reclose { window: app, app },
        Inverse::Reclose { app, .. } => Inverse::Reopen(app),
        Inverse::Grant { app, row, withheld } => Inverse::Grant {
            app,
            row,
            withheld: !withheld,
        },
        // With no model before there was nothing to undo to, so no redo either:
        // the inverse stays one that refuses.
        Inverse::Model { back: None, from } => Inverse::Model { back: None, from },
        Inverse::Model {
            back: Some(back),
            from,
        } => Inverse::Model {
            back: Some(from),
            from: back,
        },
    }
}

/// An inverse as one word, for the log and the `scriptrun` wire: `spec:1:2:3:2`,
/// `fs:12`, `none`. A held send's cancel is `none`: it lives in the shell's
/// memory only, so no log read back can cancel, or send, anything.
pub fn inverse_word(inverse: Option<Inverse>) -> String {
    match inverse {
        None | Some(Inverse::Unsend { .. }) => String::from("none"),
        Some(Inverse::Unread(raw)) => String::from(raw.as_str()),
        Some(Inverse::Spec {
            policy,
            preset,
            after,
        }) => format!("spec:{policy}:{preset}:{}:{}", after.0, after.1),
        Some(Inverse::Started(name)) => format!("started:{}", name.escaped()),
        Some(Inverse::Stopped(name)) => format!("stopped:{}", name.escaped()),
        Some(Inverse::Generation { from, to }) => format!("generation:{from}:{to}"),
        Some(Inverse::Local { from, to }) => format!("local:{from}:{to}"),
        Some(Inverse::Fs { txn }) => format!("fs:{txn}"),
        Some(Inverse::Fired {
            rule,
            action,
            app,
            undo,
        }) => format!(
            "fired:{rule}:{}:{}:{}",
            action.escaped(),
            app.escaped(),
            u8::from(undo)
        ),
        Some(Inverse::Instruction { id, made }) => format!("instruction:{id}:{}", u8::from(made)),
        Some(Inverse::Profile { key }) => format!("profile:{key}"),
        Some(Inverse::Keep { app, on }) => format!("keep:{}:{}", app.escaped(), u8::from(on)),
        Some(Inverse::Goal { id, paused }) => format!("goal:{id}:{}", u8::from(paused)),
        Some(Inverse::App { app, id }) => format!("app:{}:{id}", app.escaped()),
        Some(Inverse::Appearance {
            setting,
            before,
            after,
        }) => format!(
            "appearance:{}:{}:{}",
            setting.escaped(),
            before.escaped(),
            after.escaped()
        ),
        Some(Inverse::Refocus { to, from }) => {
            format!("refocus:{}:{}", to.escaped(), from.escaped())
        }
        Some(Inverse::Reopen(app)) => format!("reopen:{}", app.escaped()),
        Some(Inverse::Reclose { window, app }) => {
            format!("reclose:{}:{}", window.escaped(), app.escaped())
        }
        Some(Inverse::Grant { app, row, withheld }) => format!(
            "grant:{}:{}:{}",
            app.escaped(),
            row.escaped(),
            u8::from(withheld)
        ),
        Some(Inverse::NetGrant { id, held }) => format!("netgrant:{id}:{}", u8::from(held)),
        Some(Inverse::Model { back, from }) => format!(
            "model:{}:{}",
            back.map_or_else(|| String::from("none"), |b| comp_models::hex(&b)),
            comp_models::hex(&from)
        ),
        Some(Inverse::Fetched { sha }) => format!("fetched:{}", comp_models::hex(&sha)),
    }
}

/// [`inverse_word`] read back; `None` for a word it could not have written.
pub fn parse_inverse(text: &str) -> Option<Option<Inverse>> {
    let mut parts = text.split(':');
    let number = |part: Option<&str>| part?.parse::<u64>().ok();
    let inverse = match parts.next()? {
        "none" => return Some(None),
        "spec" => Inverse::Spec {
            policy: number(parts.next())?,
            preset: number(parts.next())?,
            after: (number(parts.next())?, number(parts.next())?),
        },
        "started" => Inverse::Started(Name::from_word(parts.next()?)?),
        "stopped" => Inverse::Stopped(Name::from_word(parts.next()?)?),
        "generation" => Inverse::Generation {
            from: number(parts.next())?,
            to: number(parts.next())?,
        },
        "local" => Inverse::Local {
            from: number(parts.next())?,
            to: number(parts.next())?,
        },
        "fs" => Inverse::Fs {
            txn: number(parts.next())?,
        },
        "fired" => Inverse::Fired {
            rule: number(parts.next())?,
            action: Name::from_word(parts.next()?)?,
            app: Name::from_word(parts.next()?)?,
            undo: flag(parts.next())?,
        },
        "profile" => Inverse::Profile {
            key: number(parts.next())?,
        },
        "keep" => Inverse::Keep {
            app: Name::from_word(parts.next()?)?,
            on: flag(parts.next())?,
        },
        "goal" => Inverse::Goal {
            id: number(parts.next())?,
            paused: flag(parts.next())?,
        },
        "app" => Inverse::App {
            app: Name::from_word(parts.next()?)?,
            id: number(parts.next())?,
        },
        "appearance" => Inverse::Appearance {
            setting: Name::from_word(parts.next()?)?,
            before: Name::from_word(parts.next()?)?,
            after: Name::from_word(parts.next()?)?,
        },
        "instruction" => Inverse::Instruction {
            id: number(parts.next())?,
            made: flag(parts.next())?,
        },
        "refocus" => Inverse::Refocus {
            to: Name::from_word(parts.next()?)?,
            from: Name::from_word(parts.next()?)?,
        },
        "reopen" => Inverse::Reopen(Name::from_word(parts.next()?)?),
        "reclose" => Inverse::Reclose {
            window: Name::from_word(parts.next()?)?,
            app: Name::from_word(parts.next()?)?,
        },
        "grant" => Inverse::Grant {
            app: Name::from_word(parts.next()?)?,
            row: Name::from_word(parts.next()?)?,
            withheld: flag(parts.next())?,
        },
        "netgrant" => Inverse::NetGrant {
            id: u32::try_from(number(parts.next())?).ok()?,
            held: flag(parts.next())?,
        },
        "model" => {
            let sha = |part: Option<&str>| modelchan::listing::sha256(part?).ok();
            let back = match parts.next()? {
                "none" => None,
                hex => Some(sha(Some(hex))?),
            };
            Inverse::Model {
                back,
                from: sha(parts.next())?,
            }
        }
        "fetched" => Inverse::Fetched {
            sha: modelchan::listing::sha256(parts.next()?).ok()?,
        },
        _ => return None,
    };
    // A word with parts left over was not written by `inverse_word`: a name
    // with a `:` from before names were escaped (bug-hunt-3 H3-24). It is not
    // read as other values.
    parts.next().is_none().then_some(Some(inverse))
}

/// `0` or `1`.
fn flag(part: Option<&str>) -> Option<bool> {
    match part? {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}
