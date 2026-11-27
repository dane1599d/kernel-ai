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
//! ([`Asker`]): the panel's to its bar (`take_card_result`), the Terminal's to
//! the Terminal (#922), which is told the card's menu, shows it and waits,
//! and asks what came of it (`deskcard`). The window may decline its question
//! while it waits (ADR 0005 section 7); once on the card, the card answers it.

use crate::actions::{CONFIRM_EXPIRY_NS, Pending};
use crate::deskcard::NOTHING_WAITS;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;

/// Proposals waiting for the card, every desktop line together.
pub(crate) const MAX_WAITING: usize = 4;
/// The places among [`MAX_WAITING`] agents' questions may take, so the
/// person's panel and Terminal lines keep room on the card.
pub(crate) const AGENTS_WAITING: usize = 2;
/// What an agent's question is told when agents' places are full.
const AGENTS_FULL: &str = "agents' questions fill their places on the card";
/// What the line is told when its proposal waits for the card.
pub(crate) const ON_CARD: &str = "it asks first: answer Run or Don't on the card";
/// What it is told when it is a script, which only the console shows.
pub(crate) const SCRIPT: &str =
    "it asks first, and its script shows only in the console: ask there";
/// What it is told when it is a send, whose text only the console shows.
pub(crate) const SEND: &str = "a send shows its text only in the console: ask there";
/// What it is told when a send's card and the Kai pane do not fit side by
/// side here, or an agent asks for it: the console asks (ADR 0005 §7,
/// "Amended 2026-11-19").
pub(crate) const SEND_HERE: &str =
    "it is a send, and its text cannot show beside its card here: ask in the console";
/// What it is told when too many already wait.
const FULL: &str = "it asks first, and other questions wait: answer those first";
/// What it is told when its one call is too long, or not plain ASCII, for
/// the card's menu, which then asks a y/n question only the console shows.
pub(crate) const TOO_LONG: &str =
    "it asks first, and its call is too long or not plain text for the card: ask in the console";
/// The line for a card answered Don't, or declined in its window.
pub(crate) const DECLINED: &str = "Not run: you said Don't.";
/// The line for a question that lapsed before the card answered it.
const LAPSED: &str = "Not run: it waited more than two minutes for the card.";
/// The line for a card's question answered after its two minutes: the
/// card's countdown says Deny then, and a late answer at the console finds it
/// expired too (the kernel sends the two alike), so neither ran.
const UNANSWERED: &str = "Not run: it was not answered within two minutes.";
/// The line for a question on the card that the console's own line let go.
const DROPPED: &str = "Not run: a line typed in the console let the question go.";
/// What a window's decline is told once its question is on the card.
const ON_THE_CARD: &str = "it is on the card now: answer Run or Don't there";

/// Where a desktop line was typed: where what its card came to goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Asker {
    /// The AI panel: its bar shows it (`take_card_result`).
    Panel,
    /// An app's window, the Terminal's: its `shell_card` effect's result.
    App(String),
    /// An agent's step (`agentstep`): its `agent-step wait <id>`.
    Agent(String),
}

/// A proposal from a desktop line, waiting for the card.
pub(crate) struct Waiting {
    pub pending: Pending,
    /// The menu, its origin `added`: the card's text.
    pub menu: String,
    /// A send's text, which the Kai pane shows beside its card
    /// (`Host::desk_preview`): given to `desktop` as the card goes up.
    pub send: Option<String>,
    pub asker: Asker,
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
    if text.starts_with(termcore::menu::SHOWN) {
        return (SEND, "discarded: a send shows its text only in the console");
    }
    match text.starts_with("Run ") && text.ends_with("y/n, or show") {
        true => (TOO_LONG, "discarded: the card cannot show the call"),
        false => (SCRIPT, "discarded: only the console shows a script"),
    }
}

/// The question a window's line declines: exactly the decline it sends
/// (`termcore::menu::answer_line` with `Deny`).
fn declined(line: &str) -> Option<u64> {
    let id = line.strip_prefix("answer ")?.split_once(' ')?;
    let deny = termcore::menu::Answer::Deny.word();
    (id.1 == deny).then(|| id.0.parse().ok()).flatten()
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
        // A send's card (ADR 0005 §7, "Amended 2026-11-19"): its menu goes
        // to the card plain, and its text to the Kai pane, where the card and
        // the open pane fit side by side; never an agent's.
        let shown = termcore::menu::split_shown(&reply.text).and_then(Result::ok);
        let send = match (&shown, &pending.send, &asker) {
            (Some(_), Some(send), Asker::Panel | Asker::App(_)) => Some(send.body.clone()),
            _ => None,
        };
        let fits = send.is_some() && self.host.desk_send_fits();
        let menu_text = match (&shown, fits) {
            (Some(shown), true) => shown.menu,
            _ => reply.text.as_str(),
        };
        let menu = match (reply.confirm, &pending.menu) {
            (Some(id), Some(_)) if id == pending.id => as_added(id, menu_text),
            _ => None,
        };
        // An agent's card says who asks and offers no Always.
        let was_menu = menu.is_some();
        let menu = match &asker {
            Asker::Agent(agent) => menu.and_then(|m| crate::agentconsent::agent_menu(agent, &m)),
            _ => menu,
        };
        let (why, outcome) = match menu {
            None if shown.is_some() && send.is_some() => (
                SEND_HERE,
                "discarded: its text cannot show beside its card here",
            ),
            // A menu the agent's name made too long for the card.
            None if was_menu => (
                TOO_LONG,
                "discarded: the agent's question is too long for the card",
            ),
            None => not_on_card(&reply.text),
            Some(_) if self.desk_asks.len() >= MAX_WAITING => {
                (FULL, "discarded: other questions wait")
            }
            // Agents take at most AGENTS_WAITING places: the person's own
            // lines can always reach the card.
            Some(_)
                if matches!(asker, Asker::Agent(_)) && self.agents_waiting() >= AGENTS_WAITING =>
            {
                (
                    AGENTS_FULL,
                    "discarded: agents' questions fill their places",
                )
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
                    send: send.filter(|_| fits),
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

    /// How many agents' questions wait for the card.
    fn agents_waiting(&self) -> usize {
        self.desk_asks
            .iter()
            .filter(|w| matches!(w.asker, Asker::Agent(_)))
            .count()
    }

    /// Whether a desktop line's proposal waits for the card.
    pub(crate) fn desk_ask_waiting(&self) -> bool {
        !self.desk_asks.is_empty()
    }

    /// Whether question `id` is the one on the card.
    pub(crate) fn on_card_for_desk(&self, id: u64) -> bool {
        self.card_for_desk.as_ref().is_some_and(|(on, _)| *on == id)
    }

    /// Whether the card's question `id` is past a question's two minutes, so
    /// any answer to it is refused as expired: the card's countdown, which
    /// says Deny then (it waits as long, counted from after the question was
    /// taken), or a person late at the console, whose Deny reads the same.
    pub(crate) fn card_lapsed(&self, id: u64) -> bool {
        let now = self.host.now_ns();
        self.on_card_for_desk(id)
            && self
                .pending
                .as_ref()
                .is_some_and(|p| p.id == id && now.saturating_sub(p.created_ns) > CONFIRM_EXPIRY_NS)
    }

    /// The oldest desktop line's proposal, as the card's menu: it becomes the
    /// question the answer goes to, its two minutes counted from now.
    pub(crate) fn take_desk_ask(&mut self) -> Option<Reply> {
        self.lapse_desk_asks();
        if self.desk_asks.is_empty() {
            return None;
        }
        // The person's own lines go to the card before agents' questions.
        let first = self
            .desk_asks
            .iter()
            .position(|w| !matches!(w.asker, Asker::Agent(_)))
            .unwrap_or(0);
        let Waiting {
            mut pending,
            menu,
            asker,
            send,
        } = self.desk_asks.remove(first);
        pending.created_ns = self.host.now_ns();
        let id = pending.id;
        // A send's text goes to the Kai pane first: `desktop` shows it only
        // if its hash is the card's, and the card takes no Allow until it has
        // been read through. One that does not get there leaves the card to
        // Don't or its lapse.
        if let Some(text) = send
            && let Err(why) = self.host.desk_preview(id, &text)
        {
            self.host.audit(&format!(
                "request {id}: its text did not reach the pane: {why}"
            ));
        }
        self.host.audit(&format!("request {id}: on the card"));
        self.card_for_desk = Some((id, asker));
        self.pending = Some(pending);
        Some(Reply::menu(id, menu))
    }

    /// Lets go of waiting questions older than a question's two minutes, as
    /// the console's lapse: what they would do may no longer be meant. A
    /// window's line is told.
    pub(crate) fn lapse_desk_asks(&mut self) {
        let now = self.host.now_ns();
        while self
            .desk_asks
            .first()
            .is_some_and(|w| now.saturating_sub(w.pending.created_ns) > CONFIRM_EXPIRY_NS)
        {
            let Waiting { pending, asker, .. } = self.desk_asks.remove(0);
            self.record(
                pending.id,
                &pending.request,
                &pending.checked.script,
                "expired",
            );
            match asker {
                Asker::App(app) => self.card_came_to(&app, pending.id, &Reply::ok(LAPSED)),
                Asker::Agent(agent) => {
                    self.agent_told(agent, pending.id, (false, false), String::from(LAPSED));
                }
                Asker::Panel => {}
            }
        }
    }

    /// Notes what the answer to a desktop line's card came to, for where the
    /// line was typed: `answering` is the question answered and the answer's
    /// word, `lapsed` whether it came after the question's two minutes
    /// ([`Shell::card_lapsed`]). Code's words only, as the panel's own results
    /// are: the run's result line or "Done.", a fixed line for Don't or a
    /// lapse, a fixed reason for a failure. Never the reply's text, which can
    /// be an app's.
    pub(crate) fn desk_answered(
        &mut self,
        answering: Option<(u64, &str)>,
        reply: &Reply,
        lapsed: bool,
    ) {
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
        // A yes the shell then refused (its rule not kept, its Allow not
        // checked) is logged as declined and replies ok: it ran nothing, so
        // where the line was typed it is no "Done." (#17).
        let refused = !declined && self.log.get(id).is_some_and(|e| e.outcome == "declined");
        let (ran, line) = match (lapsed, declined, reply.ok && !refused) {
            (true, _, _) => (false, UNANSWERED),
            (false, true, _) => (false, DECLINED),
            (false, false, true) => (true, ""),
            (false, false, false) => (false, crate::panel::FAILED),
        };
        // It ran, but its file changes did not land: that, never "Done".
        let not_landed = self.take_not_landed_line().filter(|_| !lapsed && !declined);
        // A send the card allowed is held, not done: its line says so, with
        // its undo, as the console's does (#1142's review), made from the
        // outbox's entry, never the reply's text.
        let held = match ran {
            true => self
                .outbox
                .get(id)
                .map(|pending| crate::outbox::held_line(&pending.send.host, agentabi::HOLD_MS, id)),
            false => None,
        };
        let line = match (ran, held) {
            (true, Some(held)) => held,
            (true, None) => self
                .done_line
                .take()
                .map(|line| crate::panel::without_undo(&line))
                .unwrap_or_else(|| String::from("Done.")),
            (false, _) => not_landed.unwrap_or_else(|| String::from(line)),
        };
        // Don't and a lapse are no failure: only a run that failed is an
        // error row.
        let failed = !ran && !declined && !lapsed;
        // An agent's step the shell refused (its Allow not checked) ran
        // nothing: that is a no for it, not a run that failed.
        let failed = failed && !(refused && matches!(asker, Asker::Agent(_)));
        self.desk_told(id, asker, ran, failed, line);
    }

    /// Question `id`, on the card, is gone without an answer: a line typed in
    /// the console let its proposal go. Where it was typed is told.
    pub(crate) fn desk_question_dropped(&mut self, id: u64) {
        if !self.on_card_for_desk(id) {
            return;
        }
        if let Some((_, asker)) = self.card_for_desk.take() {
            self.desk_told(id, asker, false, false, String::from(DROPPED));
        }
    }

    /// Tells `asker` what question `id` came to: the panel through
    /// `card_result`, a window as its `shell_card`'s result (`deskcard`).
    pub(crate) fn desk_told(
        &mut self,
        id: u64,
        asker: Asker,
        ran: bool,
        failed: bool,
        line: String,
    ) {
        match asker {
            Asker::Panel => {
                // A send the card allowed is held as the card's own entry.
                self.card_held = (ran && self.outbox.get(id).is_some()).then_some(id);
                self.card_result = Some((ran, line));
            }
            Asker::App(app) => {
                let told = match failed {
                    true => Reply::err(line),
                    false => Reply::ok(line),
                };
                self.card_came_to(&app, id, &told);
            }
            Asker::Agent(agent) => {
                // `failed` is a run the person allowed that then failed (its
                // file changes not landing too); a question let go of unrun
                // is told with `failed` false, whatever its words.
                self.agent_told(agent, id, (ran, failed), line);
            }
        }
    }

    /// What the last answered panel line's card came to, once: whether it
    /// ran, and its first line, for the panel to show.
    pub fn take_card_result(&mut self) -> Option<(bool, String)> {
        self.card_result.take()
    }

    /// The send the last answered panel line's card allowed, if the outbox
    /// holds it: the entry its undo takes back.
    pub fn take_card_held(&mut self) -> Option<u64> {
        self.card_held.take()
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
        let Some(at) = self
            .desk_asks
            .iter()
            .position(|w| w.pending.id == id && w.asker == mine)
        else {
            return Some(Reply::err(NOTHING_WAITS));
        };
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
    use super::{SCRIPT, SEND, TOO_LONG, as_added, declined, not_on_card};

    #[test]
    fn a_call_too_long_for_the_menu_is_told_so_and_a_script_is_not() {
        let long = "Run notes_add(\"a very long note\")? It asks first. y/n, or show";
        assert_eq!(not_on_card(long).0, TOO_LONG);
        assert_eq!(not_on_card("script 4:\nlet x = 1;\ny/n, or show").0, SCRIPT);
        // A send's text is shown only in the console: its own reason.
        let send = termcore::menu::encode_shown(
            "Kai wants to send this text to a.example:",
            "hi",
            "menu 5 asked\nq\n\n\nx",
        );
        assert_eq!(not_on_card(&send).0, SEND);
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
}
