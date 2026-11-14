//! A line typed at the desktop, in the AI panel or the Terminal window, that
//! asks first (the founder's VirtualBox test, 2026-11-14: nothing on the
//! desktop could approve one, so a note could not be made without the
//! kernel's console).
//!
//! Such a line's proposal is kept as a card in waiting, as an app's network
//! ask is: the kernel's terminal takes it ([`termcore::menu::TAKE_CARD`]) when
//! `desktop` shows and no other ask is up, and posts it on `desktop`'s trusted
//! ask card. Code drew the call and the question there, the person answers
//! Run or Don't on that card, and an Allow mints the `ONCE` for the call as
//! shown (ADR 0005 section 7). A window or the panel never answers it: the
//! line only puts the question in line. No typed line at the trusted terminal
//! asked for the card, so its highlight starts on Deny.
//!
//! Only a proposal shown as an Allow / Deny menu goes this way: one call, the
//! act model's rows. A Rhai script the model wrote asks a y/n question, which
//! only the console shows (the card shows one call, never a script), so such
//! a line is still refused with that reason. What came of a card's answer is
//! given to the panel, so the desktop shows it (`take_card_result`).

use crate::actions::Pending;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;

/// Proposals waiting for the card, every desktop line together.
pub(crate) const MAX_WAITING: usize = 4;
/// What the line is told when its proposal waits for the card.
pub(crate) const ON_CARD: &str = "it asks first: answer Run or Don't on the card";
/// What it is told when it is a script, which only the console shows.
pub(crate) const SCRIPT: &str =
    "it asks first, and its script shows only in the console: ask there";
/// What it is told when too many already wait.
const FULL: &str = "it asks first, and other questions wait: answer those first";
/// What it is told when its one call is too long, or not plain ASCII, for
/// the card's menu, which then asks a y/n question only the console shows.
pub(crate) const TOO_LONG: &str =
    "it asks first, and its call is too long or not plain text for the card: ask in the console";
/// The panel's line for a card answered Don't.
const DECLINED: &str = "Not run: you said Don't.";

/// A proposal from a desktop line, waiting for the card.
pub(crate) struct Waiting {
    pub pending: Pending,
    /// The menu, its origin `added`: the card's text.
    pub menu: String,
}

/// `text`, a menu reply's text, with its origin `added`: no typed line at the
/// trusted terminal asked for this card. `None` for anything not a menu.
fn as_added(id: u64, text: &str) -> Option<String> {
    let (head, rest) = text.split_once('\n')?;
    let asked = format!("menu {id} asked");
    let added = format!("menu {id} added");
    match head {
        h if h == asked || h == added => Some(format!("{added}\n{rest}")),
        _ => None,
    }
}

/// Why a question that is not a menu cannot go on the card, and its log
/// outcome: one call too long or not plain text for the menu asks a y/n
/// question instead (`agentcore`'s fallback), anything else is a script.
fn not_on_card(text: &str) -> (&'static str, &'static str) {
    match text.starts_with("Run ") && text.ends_with("y/n, or show") {
        true => (TOO_LONG, "discarded: the card cannot show the call"),
        false => (SCRIPT, "discarded: only the console shows a script"),
    }
}

impl Shell {
    /// Puts the proposal the last line left in [`Shell::pending`] in line for
    /// the card, if it is one the card can show; why not, otherwise. The
    /// pending proposal is taken either way.
    pub(crate) fn wait_for_card(&mut self, reply: &Reply) -> Result<&'static str, &'static str> {
        let Some(pending) = self.pending.take() else {
            return Err(SCRIPT);
        };
        self.lapse_desk_asks();
        let menu = match (reply.confirm, &pending.menu) {
            (Some(id), Some(_)) if id == pending.id => as_added(id, &reply.text),
            _ => None,
        };
        let (why, outcome) = match menu {
            None => not_on_card(&reply.text),
            Some(_) if self.desk_asks.len() >= MAX_WAITING => {
                (FULL, "discarded: other questions wait")
            }
            Some(menu) => {
                self.host
                    .audit(&format!("request {}: waits for the card", pending.id));
                self.desk_asks.push(Waiting { pending, menu });
                return Ok(ON_CARD);
            }
        };
        self.record(
            pending.id,
            &pending.request,
            &pending.checked.script,
            outcome,
        );
        self.save_log();
        Err(why)
    }

    /// Whether a desktop line's proposal waits for the card.
    pub(crate) fn desk_ask_waiting(&self) -> bool {
        !self.desk_asks.is_empty()
    }

    /// The oldest desktop line's proposal, as the card's menu: it becomes the
    /// question the answer goes to, its two minutes counted from now.
    pub(crate) fn take_desk_ask(&mut self) -> Option<Reply> {
        self.lapse_desk_asks();
        if self.desk_asks.is_empty() {
            return None;
        }
        let Waiting { mut pending, menu } = self.desk_asks.remove(0);
        pending.created_ns = self.host.now_ns();
        let id = pending.id;
        self.host.audit(&format!("request {id}: on the card"));
        self.card_for_desk = Some(id);
        self.pending = Some(pending);
        Some(Reply::menu(id, menu))
    }

    /// Lets go of waiting questions older than a question's two minutes, as
    /// the console's lapse: what they would do may no longer be meant.
    fn lapse_desk_asks(&mut self) {
        let now = self.host.now_ns();
        while self.desk_asks.first().is_some_and(|w| {
            now.saturating_sub(w.pending.created_ns) > crate::actions::CONFIRM_EXPIRY_NS
        }) {
            let old = self.desk_asks.remove(0).pending;
            self.record(old.id, &old.request, &old.checked.script, "expired");
        }
    }

    /// Notes what the answer to a desktop line's card came to, for the
    /// panel: `answering` is the question answered and the answer's word.
    /// Code's words only, as the panel's own results are: the run's result
    /// line or "Done.", a fixed line for Don't, a fixed reason for a failure.
    /// Never the reply's text, which can be an app's.
    pub(crate) fn desk_answered(&mut self, answering: Option<(u64, &str)>, reply: &Reply) {
        let Some((id, word)) = answering else {
            return;
        };
        if Some(id) != self.card_for_desk || self.pending.is_some() {
            return;
        }
        self.card_for_desk = None;
        let declined = word == termcore::menu::Answer::Deny.word();
        self.card_result = Some(match (declined, reply.ok) {
            (true, _) => (false, String::from(DECLINED)),
            (false, true) => (
                true,
                self.done_line
                    .take()
                    .map(|line| crate::panel::without_undo(&line))
                    .unwrap_or_else(|| String::from("Done.")),
            ),
            (false, false) => (false, String::from(crate::panel::FAILED)),
        });
    }

    /// What the last answered desktop card came to, once: whether it ran,
    /// and its first line, for the panel to show.
    pub fn take_card_result(&mut self) -> Option<(bool, String)> {
        self.card_result.take()
    }
}

#[cfg(test)]
mod tests {
    use super::{SCRIPT, TOO_LONG, as_added, not_on_card};

    #[test]
    fn a_call_too_long_for_the_menu_is_told_so_and_a_script_is_not() {
        let long = "Run notes_add(\"a very long note\")? It asks first. y/n, or show";
        assert_eq!(not_on_card(long).0, TOO_LONG);
        assert_eq!(not_on_card("script 4:\nlet x = 1;\ny/n, or show").0, SCRIPT);
    }

    #[test]
    fn a_menu_is_marked_added_and_anything_else_is_not_a_menu() {
        assert_eq!(
            as_added(7, "menu 7 asked\nRun x()?\n\n\nx()").as_deref(),
            Some("menu 7 added\nRun x()?\n\n\nx()")
        );
        assert_eq!(
            as_added(7, "menu 7 added\nq\n\n\nx()").as_deref(),
            Some("menu 7 added\nq\n\n\nx()")
        );
        assert_eq!(as_added(8, "menu 7 asked\nq"), None);
        assert_eq!(as_added(7, "Run x()? y/n, or show"), None);
        assert_eq!(as_added(7, "menu 7 asked"), None);
    }
}
