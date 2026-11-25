//! An ask-tier call's card: the Allow / Deny menu, and the reasons it gives.

use super::{Arguments, read_from};
use crate::actions::How;
use crate::actlog::Form;
use crate::rules::Offer;
use crate::script::{self, Checked};
use crate::tiers::Tier;
use crate::{Reply, Shell};
use agentabi::consent::Why;
use agentabi::{Action, Role};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use termcore::menu::{Menu, Origin};

impl Shell {
    /// An ask-tier call waits for an answer from the Allow / Deny menu, or for `y`
    /// or `n` when its question cannot be shown as a menu.
    pub(crate) fn ask_call(
        &mut self,
        id: u64,
        request: &str,
        checked: Checked,
        why: Why,
        offer: Option<Offer>,
        subject: Option<String>,
    ) -> Reply {
        self.ask_call_with(id, request, checked, why, offer, subject, None)
    }

    /// [`Shell::ask_call`], with the row's own reason when it has one (a
    /// held send's), in place of the one `why` gives.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ask_call_with(
        &mut self,
        id: u64,
        request: &str,
        checked: Checked,
        why: Why,
        offer: Option<Offer>,
        subject: Option<String>,
        own: Option<String>,
    ) -> Reply {
        let reason = match why {
            // Consent's own reasons stand: the hold is not why it asks then.
            Why::Steered(_) | Why::Outbound(_) | Why::Added | Why::NotEnglish | Why::Ungrounded => {
                reason_of(why)
            }
            _ => own.unwrap_or_else(|| reason_of(why)),
        };
        let call = checked.script.trim_end_matches(';').to_string();
        let changes: Vec<&'static str> = checked.changes.clone();
        self.shown = Some(format!(
            "{}\nasked first: {reason}",
            script::shown(id, &checked)
        ));
        self.pending = Some(crate::actions::Pending {
            id,
            request: request.to_string(),
            checked,
            asking: changes,
            created_ns: self.host.now_ns(),
            how: How {
                form: Form::Call,
                tier: Tier::Ask,
                consent: String::from("asked"),
            },
            call: None,
            offer: offer.clone(),
            menu: None,
            pkg: None,
            standing: None,
            model: None,
            app: None,
            install: None,
            net: None,
            send: None,
            power: false,
            goal: None,
        });
        // The highlight starts on Allow only for a call the line plainly asked
        // for: one with no undo, an ungrounded fetch, or one the line asked for
        // that asks anyway (a model fetch, for the gigabytes it brings).
        let origin = match why {
            Why::NoUndo | Why::Ungrounded | Why::Asked => Origin::Asked,
            _ => Origin::Added,
        };
        let (kind, target) = offer.map(|o| o.labels()).unwrap_or_default();
        let question = match subject {
            // The call stays word for word: the ONCE an Allow mints names it.
            Some(subject) => format!("{subject} with {call}? {}.", capitalised(&reason)),
            None => format!("Run {call}? {}.", capitalised(&reason)),
        };
        let menu = Menu::new(id, &question, Some(&kind), Some(&target), origin)
            .and_then(|menu| menu.with_call(&call));
        match menu {
            Ok(_) => {
                let text = Menu::encode(id, origin, &question, &kind, &target, &call);
                if let Some(pending) = &mut self.pending {
                    pending.menu = Some(call);
                }
                Reply::menu(id, text)
            }
            // Too long, or not plain ASCII, for the menu: the y/n question shows it.
            Err(_) => Reply {
                ok: true,
                text: format!("Run {call}? {reason}. y/n, or show"),
                confirm: Some(id),
                more: false,
            },
        }
    }
}

/// Why a call asks, as its card says it.
pub(crate) fn reason_of(why: Why) -> String {
    match why {
        Why::NotEnglish => String::from("the request is not in English, so it asks first"),
        Why::Added => String::from("the request did not ask for it"),
        Why::NoUndo => String::from("it has no undo"),
        Why::Steered(source) => format!("Kai took it from {}", read_from(source)),
        Why::Outbound(source) => format!(
            "it would send what Kai read from {} out of this machine",
            read_from(source)
        ),
        Why::Ungrounded => String::from("the request did not name that site"),
        why => grant_reason(why).unwrap_or_else(|| String::from("it asks first")),
    }
}

/// Whether `row` is a send held with `undo` after its Allow (`Hold`).
pub(super) fn held(row: &Action) -> bool {
    matches!(row.inverse, agentabi::Inverse::Hold { .. })
}

/// A held send's own reason on its card: it does have an undo, for a while.
pub(crate) fn hold_reason(row: &Action) -> Option<String> {
    match row.inverse {
        agentabi::Inverse::Hold { ms } => Some(format!(
            "after you allow, it waits {} s so you can undo",
            ms.div_ceil(1000)
        )),
        _ => None,
    }
}

/// The standing rules a menu may offer for a call that asks for `why`: the row's
/// own ask (no undo) takes a rule for the action or for its target; a host nobody
/// named takes one for that host only. Asks from consent take none, and a held
/// send never takes one: a send always asks (ADR 0002 question 3).
pub(super) fn offer(row: &'static Action, args: &Arguments, why: Why) -> Option<Offer> {
    if held(row) {
        return None;
    }
    let target = row
        .args
        .iter()
        .find(|arg| arg.role == Role::Target)
        .and_then(|arg| args.values.iter().find(|(name, _)| *name == arg.name))
        .map(|(_, text)| text.clone());
    match why {
        Why::NoUndo => Some(Offer {
            action: row.name,
            target,
            kind: true,
        }),
        Why::Ungrounded => target.map(|target| Offer {
            action: row.name,
            target: Some(target),
            kind: false,
        }),
        _ => None,
    }
}

/// Why a read from a site only an app's grant grounds asks
/// (`agentabi::consent_grant`): what decided it, never that it sends.
pub(crate) fn grant_reason(why: Why) -> Option<String> {
    Some(match why {
        Why::GrantSteered(source) => format!(
            "{} chose this read from a site allowed for an app, not the request",
            read_from(source)
        ),
        Why::GrantPath => {
            String::from("the request did not name that site, an app's, or what to read from it")
        }
        Why::GrantOffline => String::from("the request said not to go online"),
        Why::GrantMany => String::from(
            "it would read from a site allowed for an app more than once, which the request did not ask for",
        ),
        _ => return None,
    })
}

/// `text` with its first letter in capitals, to start the menu's sentence.
pub(super) fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
