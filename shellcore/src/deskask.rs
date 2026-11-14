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
//! a line is still refused with that reason.
//!
//! What came of a card's answer goes back where its line was typed
//! ([`Asker`]): the panel's to the panel (`take_card_result`), so its bar
//! shows it; the Terminal's to the Terminal (AP-2's founder-walk, #922: it
//! went to the bar and the Terminal never heard). The Terminal's line is told
//! the card's menu, shows its question and waits, and asks what came of it
//! with one more effect, [`SHELL_CARD`]: the shell holds that effect and
//! answers it with the card's answer, as code's words. Each effect gets one
//! result, so the line's own result (the menu) and the answer each have one.
//! The window may decline its question while it waits for the card, the one
//! answer a window may give (ADR 0005 section 7); once it is on the card, the
//! card answers it.

use crate::actions::Pending;
use crate::effects::EffectDone;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use appabi::{Ty, Val, wave};

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
/// The line for a card answered Don't, or declined in its window.
const DECLINED: &str = "Not run: you said Don't.";
/// The line for a question that lapsed before the card answered it.
const LAPSED: &str = "Not run: it waited more than two minutes for the card.";
/// What a window's decline is told once its question is on the card.
const ON_THE_CARD: &str = "it is on the card now: answer Run or Don't there";
/// The effect a window queues once its line's question waits for the card:
/// its result is what the card came to. Like `shell_line`, it is no registry
/// row, and only the Terminal may send it.
pub(crate) const SHELL_CARD: &str = "shell_card";
/// The window's answers kept until it asks, and its asks held until the card
/// answers, at most each: past it the oldest go.
const MAX_KEPT: usize = 8;

/// Where a desktop line was typed: where what its card came to goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Asker {
    /// The AI panel: its bar shows it (`take_card_result`).
    Panel,
    /// An app's window, the Terminal's: its [`SHELL_CARD`] effect's result.
    App(String),
}

/// A proposal from a desktop line, waiting for the card.
pub(crate) struct Waiting {
    pub pending: Pending,
    /// The menu, its origin `added`: the card's text.
    pub menu: String,
    pub asker: Asker,
}

/// What came of windows' questions, matched to their [`SHELL_CARD`] effects
/// in whichever order the two come.
#[derive(Default)]
pub(crate) struct Answers {
    /// (app, question, ticket): an effect that waits for the card's answer.
    held: Vec<(String, u64, u32)>,
    /// (app, question, wire reply): an answer that waits for the effect.
    came: Vec<(String, u64, String)>,
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

/// The question of `shell_card({id: ...})`.
fn id_of(args: &str) -> Result<u64, String> {
    let ty = Ty::Record(alloc::vec![(String::from("id"), Ty::U64)]);
    match wave::parse(args, &ty) {
        Ok(Val::Record(fields)) => match fields.as_slice() {
            [Val::U64(id)] => Ok(*id),
            _ => Err(format!("{SHELL_CARD} takes {{id}}")),
        },
        Ok(_) => Err(format!("{SHELL_CARD} takes {{id}}")),
        Err(why) => Err(format!("{SHELL_CARD}'s arguments: {why}")),
    }
}

/// The question a window's line declines: exactly the decline it sends
/// (`termcore::menu::answer_line` with `Deny`).
fn declined(line: &str) -> Option<u64> {
    let id = line.strip_prefix("answer ")?.split_once(' ')?;
    let deny = termcore::menu::Answer::Deny.word();
    (id.1 == deny).then(|| id.0.parse().ok()).flatten()
}

/// Pushes `item` on `list`, the oldest going past [`MAX_KEPT`]: what went.
fn kept<T>(list: &mut Vec<T>, item: T) -> Option<T> {
    let gone = (list.len() == MAX_KEPT).then(|| list.remove(0));
    list.push(item);
    gone
}

impl Shell {
    /// Puts the proposal the last line left in [`Shell::pending`] in line for
    /// the card, if it is one the card can show: the card's menu, for a
    /// window to show; why not, otherwise. The pending proposal is taken
    /// either way. `asker` is where what the card comes to goes.
    pub(crate) fn wait_for_card(
        &mut self,
        reply: &Reply,
        asker: Asker,
    ) -> Result<Reply, &'static str> {
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
                let id = pending.id;
                self.host
                    .audit(&format!("request {id}: waits for the card"));
                let shown = Reply::menu(id, menu.clone());
                self.desk_asks.push(Waiting {
                    pending,
                    menu,
                    asker,
                });
                return Ok(shown);
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

    /// Whether question `id` is the one on the card.
    pub(crate) fn on_card_for_desk(&self, id: u64) -> bool {
        self.card_for_desk.as_ref().is_some_and(|(on, _)| *on == id)
    }

    /// The oldest desktop line's proposal, as the card's menu: it becomes the
    /// question the answer goes to, its two minutes counted from now.
    pub(crate) fn take_desk_ask(&mut self) -> Option<Reply> {
        self.lapse_desk_asks();
        if self.desk_asks.is_empty() {
            return None;
        }
        let Waiting {
            mut pending,
            menu,
            asker,
        } = self.desk_asks.remove(0);
        pending.created_ns = self.host.now_ns();
        let id = pending.id;
        self.host.audit(&format!("request {id}: on the card"));
        self.card_for_desk = Some((id, asker));
        self.pending = Some(pending);
        Some(Reply::menu(id, menu))
    }

    /// Lets go of waiting questions older than a question's two minutes, as
    /// the console's lapse: what they would do may no longer be meant. A
    /// window's line is told.
    fn lapse_desk_asks(&mut self) {
        let now = self.host.now_ns();
        while self.desk_asks.first().is_some_and(|w| {
            now.saturating_sub(w.pending.created_ns) > crate::actions::CONFIRM_EXPIRY_NS
        }) {
            let Waiting { pending, asker, .. } = self.desk_asks.remove(0);
            self.record(
                pending.id,
                &pending.request,
                &pending.checked.script,
                "expired",
            );
            if let Asker::App(app) = asker {
                self.card_came_to(&app, pending.id, &Reply::ok(LAPSED));
            }
        }
    }

    /// Notes what the answer to a desktop line's card came to, for where the
    /// line was typed: `answering` is the question answered and the answer's
    /// word. Code's words only, as the panel's own results are: the run's
    /// result line or "Done.", a fixed line for Don't, a fixed reason for a
    /// failure. Never the reply's text, which can be an app's.
    pub(crate) fn desk_answered(&mut self, answering: Option<(u64, &str)>, reply: &Reply) {
        let Some((id, word)) = answering else {
            return;
        };
        if !self.on_card_for_desk(id) || self.pending.is_some() {
            return;
        }
        let Some((_, asker)) = self.card_for_desk.take() else {
            return;
        };
        let declined = word == termcore::menu::Answer::Deny.word();
        let (ran, line) = match (declined, reply.ok) {
            (true, _) => (false, String::from(DECLINED)),
            (false, true) => (
                true,
                self.done_line
                    .take()
                    .map(|line| crate::panel::without_undo(&line))
                    .unwrap_or_else(|| String::from("Done.")),
            ),
            (false, false) => (false, String::from(crate::panel::FAILED)),
        };
        match asker {
            Asker::Panel => self.card_result = Some((ran, line)),
            // Don't is no failure: only a run that failed is an error row.
            Asker::App(app) => {
                let told = match ran || declined {
                    true => Reply::ok(line),
                    false => Reply::err(line),
                };
                self.card_came_to(&app, id, &told);
            }
        }
    }

    /// What the last answered panel line's card came to, once: whether it
    /// ran, and its first line, for the panel to show.
    pub fn take_card_result(&mut self) -> Option<(bool, String)> {
        self.card_result.take()
    }

    /// `app`'s question `id` came to `told`: the result of its held
    /// [`SHELL_CARD`] effect, given back as the apps' calls' results are
    /// (`take_effect_results`), or kept until it asks.
    fn card_came_to(&mut self, app: &str, id: u64, told: &Reply) {
        let wire = crate::appline::one_line(&told.to_wire());
        let mut tickets = Vec::new();
        self.card_answers.held.retain(|(a, q, ticket)| {
            let mine = a == app && *q == id;
            if mine {
                tickets.push(*ticket);
            }
            !mine
        });
        if tickets.is_empty() {
            kept(&mut self.card_answers.came, (String::from(app), id, wire));
            return;
        }
        let done = tickets
            .into_iter()
            .map(|ticket| EffectDone {
                ticket,
                outcome: Ok(wire.clone()),
            })
            .collect();
        self.give_back(app, done);
    }

    /// Hands `done` to the host for app `app`, as the apps' calls' effects'
    /// results go.
    fn give_back(&mut self, app: &str, done: Vec<EffectDone>) {
        let dropped = self
            .host
            .apps()
            .and_then(|apps| apps.borrow_mut().keep_results(app, done));
        if let Some(dropped) = dropped {
            self.host.audit(&format!(
                "{dropped}'s effect results were dropped: the queue is full"
            ));
        }
    }

    /// `shell_card` for `app` with `ticket`: what came of its line's question
    /// on the card, if the card answered it; `None` while it waits, the
    /// effect held until it is answered.
    pub(crate) fn shell_card(
        &mut self,
        app: &str,
        ticket: u32,
        args: &str,
    ) -> Option<Result<String, String>> {
        if app != crate::appline::TERMINAL_APP {
            return Some(Err(format!("{app} may not ask what came of a card")));
        }
        let id = match id_of(args) {
            Ok(id) => id,
            Err(why) => return Some(Err(why)),
        };
        let came = self
            .card_answers
            .came
            .iter()
            .position(|(a, q, _)| a == app && *q == id);
        if let Some(at) = came {
            return Some(Ok(self.card_answers.came.remove(at).2));
        }
        let mine = Asker::App(String::from(app));
        let waits = self
            .desk_asks
            .iter()
            .any(|w| w.pending.id == id && w.asker == mine)
            || self.card_for_desk == Some((id, mine));
        if !waits {
            return Some(Err(format!("question {id} of {app}'s waits for no card")));
        }
        let gone = kept(&mut self.card_answers.held, (String::from(app), id, ticket));
        if let Some((app, _, ticket)) = gone {
            let done = alloc::vec![EffectDone {
                ticket,
                outcome: Err(String::from("too many questions wait for the card")),
            }];
            self.give_back(&app, done);
        }
        None
    }

    /// `app`'s line `line` declining its own question, which only waits for
    /// the card: let go of, and the window told "Not run" as its result.
    /// Once on the card, the card answers it. `None` for any other line.
    pub(crate) fn declined_in_window(&mut self, app: &str, line: &str) -> Option<Reply> {
        let id = declined(line)?;
        let mine = Asker::App(String::from(app));
        if self.card_for_desk == Some((id, mine.clone())) {
            // The console's own line since let its proposal go: the card
            // answers nothing now, so the window's wait ends here.
            if self.pending.as_ref().is_some_and(|p| p.id == id) {
                return Some(Reply::err(ON_THE_CARD));
            }
            self.card_for_desk = None;
            self.card_came_to(app, id, &Reply::ok(DECLINED));
            return Some(Reply::ok(""));
        }
        let at = self
            .desk_asks
            .iter()
            .position(|w| w.pending.id == id && w.asker == mine)?;
        let Waiting { pending, .. } = self.desk_asks.remove(at);
        let outcome = "declined in the window";
        self.record(
            pending.id,
            &pending.request,
            &pending.checked.script,
            outcome,
        );
        self.save_log();
        self.card_came_to(app, id, &Reply::ok(DECLINED));
        // The answer is the question's result: the decline itself says nothing.
        Some(Reply::ok(""))
    }
}

#[cfg(test)]
mod tests {
    use super::{SCRIPT, TOO_LONG, as_added, declined, id_of, not_on_card};

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

    #[test]
    fn only_the_decline_a_window_sends_reads_as_one() {
        let deny = termcore::menu::answer_line(9, termcore::menu::Answer::Deny);
        assert_eq!(declined(&deny), Some(9));
        assert_eq!(declined("answer 9 allow"), None);
        assert_eq!(declined("answer 9 deny once=3"), None);
        assert_eq!(declined("answer x deny"), None);
        assert_eq!(declined("answer 9"), None);
    }

    #[test]
    fn a_card_effect_names_one_question() {
        assert_eq!(id_of("{id: 12}"), Ok(12));
        assert!(id_of("{id: \"12\"}").is_err());
        assert!(id_of("{line: \"x\"}").is_err());
    }
}
