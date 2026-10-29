//! What a script changed, recorded as it happens with the value it replaced, so
//! `undo` can put it back. A change with no recorded inverse cannot be run-tier.

use crate::tune::spec_of;
use crate::{Host, Proof};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
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
    /// Put app `app`'s state back from before its call kept as `id` (ADR 0003
    /// §4's checkpoint undo, `kai-undo 1`): the app runs its own inverse.
    App { app: Name, id: u64 },
    /// Put keyboard focus back on `to` (`w2`, `panel` or `none`); its redo
    /// focuses `from` again (P3 task 10's `ui_focus`).
    Refocus { to: Name, from: Name },
    /// Open the app whose window `window_close` closed.
    Reopen(Name),
    /// Close `window`, which `ui_open` opened for `app`. It asks first, since
    /// the window may hold work by now.
    Reclose { window: Name, app: Name },
    /// Withhold `row` from `app`'s effects again (`withheld`), or give it back:
    /// the undo of `grant_restore` and of `grant_revoke` (P4 task 7).
    Grant {
        app: Name,
        row: Name,
        withheld: bool,
    },
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
}

/// Why a change was not put back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Something else changed it since, to this (in the user's words); putting the
    /// old value back would overwrite that, so the user is asked first.
    Moved(String),
    /// It could not be put back yet.
    Failed(String),
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
        Inverse::Generation { to, .. } => format!("generation {to}"),
        Inverse::Fs { .. } => String::from("the files as they were"),
        Inverse::Fired { app, .. } => format!("{} as it was", app.as_str()),
        Inverse::Instruction { id, made: true } => format!("standing instruction S{id} removed"),
        Inverse::Instruction { id, made: false } => format!("standing instruction S{id}"),
        Inverse::Profile { .. } => String::from("the profile before"),
        Inverse::App { app, .. } => format!("{} as it was", app.as_str()),
        Inverse::Refocus { to, .. } => format!("focus on {}", to.as_str()),
        Inverse::Reopen(app) => format!("{} open", app.as_str()),
        Inverse::Reclose { app, .. } => format!("{} closed", app.as_str()),
        Inverse::Grant { app, row, withheld } => grant_words(app, row, withheld),
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
        Inverse::Fs { txn } => match host.fs_undo(txn, anyway) {
            Ok(redo) => Ok((
                String::from("files back as they were"),
                Inverse::Fs { txn: redo },
            )),
            Err(crate::FsRefused::Later(what)) => Err(Refused::Moved(what)),
            Err(crate::FsRefused::Failed(why)) => Err(Refused::Failed(why)),
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
        Inverse::Generation { .. } => Err(Refused::Failed(String::from(
            "a package switch is put back by the shell's store",
        ))),
        Inverse::Fs { .. }
        | Inverse::Fired { .. }
        | Inverse::Refocus { .. }
        | Inverse::Reopen(_)
        | Inverse::Reclose { .. }
        | Inverse::App { .. } => Err(Refused::Failed(String::from("it is put back by revert"))),
        // Setting it twice changes nothing, so this needs no compare.
        Inverse::Grant { app, row, withheld } => {
            crate::grants::set(host, app.as_str(), row.as_str(), withheld)
                .map(|_| grant_words(app, row, withheld))
                .map_err(Refused::Failed)
        }
        Inverse::Instruction { .. } | Inverse::Profile { .. } => Err(Refused::Failed(
            String::from("it is put back by the shell's own store"),
        )),
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
    }
}

/// An inverse as one word, for the log and the `scriptrun` wire: `spec:1:2:3:2`,
/// `fs:12`, `none`.
pub fn inverse_word(inverse: Option<Inverse>) -> String {
    match inverse {
        None => String::from("none"),
        Some(Inverse::Spec {
            policy,
            preset,
            after,
        }) => format!("spec:{policy}:{preset}:{}:{}", after.0, after.1),
        Some(Inverse::Started(name)) => format!("started:{}", name.as_str()),
        Some(Inverse::Stopped(name)) => format!("stopped:{}", name.as_str()),
        Some(Inverse::Generation { from, to }) => format!("generation:{from}:{to}"),
        Some(Inverse::Fs { txn }) => format!("fs:{txn}"),
        Some(Inverse::Fired {
            rule,
            action,
            app,
            undo,
        }) => format!(
            "fired:{rule}:{}:{}:{}",
            action.as_str(),
            app.as_str(),
            u8::from(undo)
        ),
        Some(Inverse::Instruction { id, made }) => format!("instruction:{id}:{}", u8::from(made)),
        Some(Inverse::Profile { key }) => format!("profile:{key}"),
        Some(Inverse::App { app, id }) => format!("app:{}:{id}", app.as_str()),
        Some(Inverse::Refocus { to, from }) => {
            format!("refocus:{}:{}", to.as_str(), from.as_str())
        }
        Some(Inverse::Reopen(app)) => format!("reopen:{}", app.as_str()),
        Some(Inverse::Reclose { window, app }) => {
            format!("reclose:{}:{}", window.as_str(), app.as_str())
        }
        Some(Inverse::Grant { app, row, withheld }) => format!(
            "grant:{}:{}:{}",
            app.as_str(),
            row.as_str(),
            u8::from(withheld)
        ),
    }
}

/// [`inverse_word`] read back; `None` for a word it could not have written.
pub fn parse_inverse(text: &str) -> Option<Option<Inverse>> {
    let mut parts = text.split(':');
    let number = |part: Option<&str>| part?.parse::<u64>().ok();
    Some(Some(match parts.next()? {
        "none" => return Some(None),
        "spec" => Inverse::Spec {
            policy: number(parts.next())?,
            preset: number(parts.next())?,
            after: (number(parts.next())?, number(parts.next())?),
        },
        "started" => Inverse::Started(Name::new(parts.next()?)?),
        "stopped" => Inverse::Stopped(Name::new(parts.next()?)?),
        "generation" => Inverse::Generation {
            from: number(parts.next())?,
            to: number(parts.next())?,
        },
        "fs" => Inverse::Fs {
            txn: number(parts.next())?,
        },
        "fired" => Inverse::Fired {
            rule: number(parts.next())?,
            action: Name::new(parts.next()?)?,
            app: Name::new(parts.next()?)?,
            undo: flag(parts.next())?,
        },
        "profile" => Inverse::Profile {
            key: number(parts.next())?,
        },
        "app" => Inverse::App {
            app: Name::new(parts.next()?)?,
            id: number(parts.next())?,
        },
        "instruction" => Inverse::Instruction {
            id: number(parts.next())?,
            made: flag(parts.next())?,
        },
        "refocus" => Inverse::Refocus {
            to: Name::new(parts.next()?)?,
            from: Name::new(parts.next()?)?,
        },
        "reopen" => Inverse::Reopen(Name::new(parts.next()?)?),
        "reclose" => Inverse::Reclose {
            window: Name::new(parts.next()?)?,
            app: Name::new(parts.next()?)?,
        },
        "grant" => Inverse::Grant {
            app: Name::new(parts.next()?)?,
            row: Name::new(parts.next()?)?,
            withheld: flag(parts.next())?,
        },
        _ => return None,
    }))
}

/// `0` or `1`.
fn flag(part: Option<&str>) -> Option<bool> {
    match part? {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}
