//! What came of a window's question on the card, for the window (#922). The
//! Terminal shows the card's menu and asks with one more effect,
//! [`SHELL_CARD`], which the shell holds until the card answers, then answers
//! in code's words ([`Shell::take_card_answers`]): one result per effect,
//! given as a person's result is, so a long line is not cut as an app's.
//!
//! An answer is kept for [`KEPT_NS`], and each ask for its question in that
//! time hears it, answered on the same queue: a Terminal that napped or
//! reloaded while it waited asks again, and a second restore's ask hears it
//! as the first's did (#928's review).

use crate::deskask::Asker;
use crate::effects::EffectDone;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use appabi::{Ty, Val, wave};

/// The effect a window queues once its line's question waits for the card:
/// its result is what the card came to. Like `shell_line`, it is no registry
/// row, and only the Terminal may send it.
pub(crate) const SHELL_CARD: &str = "shell_card";
/// The window's answers kept, its asks held, and its results not yet given,
/// at most each: past it the oldest go.
pub(crate) const MAX_KEPT: usize = 8;
/// How long an answer is kept for a window's asks: long enough for a
/// Terminal that napped or reloaded to wake and ask again.
pub(crate) const KEPT_NS: u64 = 10 * 60 * 1_000_000_000;
/// What a window is told when nothing waits as its question: most likely the
/// shell restarted since it asked.
pub(crate) const NOTHING_WAITS: &str =
    "nothing waits for the card under that question: if the shell restarted, ask again";

/// An answer kept for a window's asks: its app, its question, its wire
/// reply, and when it came.
struct Came {
    app: String,
    id: u64,
    wire: String,
    at_ns: u64,
}

/// What came of windows' questions, matched to their [`SHELL_CARD`] effects
/// in whichever order the two come.
#[derive(Default)]
pub(crate) struct Answers {
    /// (app, question, ticket): an effect that waits for the card's answer.
    held: Vec<(String, u64, u32)>,
    /// Answers kept for the windows' asks, for [`KEPT_NS`] each.
    came: Vec<Came>,
    /// (app, result): results for held effects, for the host to give back.
    given: Vec<(String, EffectDone)>,
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

/// Pushes `item` on `list`, the oldest going past [`MAX_KEPT`]: what went.
fn kept<T>(list: &mut Vec<T>, item: T) -> Option<T> {
    let gone = (list.len() == MAX_KEPT).then(|| list.remove(0));
    list.push(item);
    gone
}

impl Shell {
    /// The results of windows' held [`SHELL_CARD`] effects, as (app, results),
    /// taken once, for the host to give back as a person's results are: a
    /// Terminal line's answer, cut as `shell_line`'s answer is.
    pub fn take_card_answers(&mut self) -> Vec<(String, Vec<EffectDone>)> {
        let mut out: Vec<(String, Vec<EffectDone>)> = Vec::new();
        for (app, done) in core::mem::take(&mut self.card_answers.given) {
            match out.iter_mut().find(|(a, _)| *a == app) {
                Some((_, batch)) => batch.push(done),
                None => out.push((app, alloc::vec![done])),
            }
        }
        out
    }

    /// `app`'s question `id` came to `told`: the result of each held
    /// [`SHELL_CARD`] effect for it, and kept for the window's asks.
    pub(crate) fn card_came_to(&mut self, app: &str, id: u64, told: &Reply) {
        let wire = crate::appline::one_line(&told.to_wire());
        let mut tickets = Vec::new();
        self.card_answers.held.retain(|(a, q, ticket)| {
            let mine = a == app && *q == id;
            if mine {
                tickets.push(*ticket);
            }
            !mine
        });
        for ticket in tickets {
            self.give_back(app, ticket, Ok(wire.clone()));
        }
        let came = Came {
            app: String::from(app),
            id,
            wire,
            at_ns: self.host.now_ns(),
        };
        kept(&mut self.card_answers.came, came);
    }

    /// Queues `outcome` as the result of `app`'s effect `ticket`.
    fn give_back(&mut self, app: &str, ticket: u32, outcome: Result<String, String>) {
        let done = EffectDone { ticket, outcome };
        if let Some((app, gone)) = kept(&mut self.card_answers.given, (String::from(app), done)) {
            self.host.audit(&format!(
                "{app}'s result for effect {} was dropped: too many wait",
                gone.ticket
            ));
        }
    }

    /// `shell_card` for `app` with `ticket`: `None` when its result goes with
    /// the card's answers ([`Shell::take_card_answers`]), at once for a
    /// question the card answered, else once it does; why not otherwise.
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
        let now = self.host.now_ns();
        self.card_answers
            .came
            .retain(|came| now.saturating_sub(came.at_ns) <= KEPT_NS);
        let came = self
            .card_answers
            .came
            .iter()
            .find(|came| came.app == app && came.id == id)
            .map(|came| came.wire.clone());
        if let Some(wire) = came {
            self.give_back(app, ticket, Ok(wire));
            return None;
        }
        let mine = Asker::App(String::from(app));
        let waits = self
            .desk_asks
            .iter()
            .any(|w| w.pending.id == id && w.asker == mine)
            || self.card_for_desk == Some((id, mine));
        if !waits {
            return Some(Err(String::from(NOTHING_WAITS)));
        }
        let held = (String::from(app), id, ticket);
        if let Some((app, _, ticket)) = kept(&mut self.card_answers.held, held) {
            let why = String::from("too many questions wait for the card");
            self.give_back(&app, ticket, Err(why));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::id_of;

    #[test]
    fn a_card_effect_names_one_question() {
        assert_eq!(id_of("{id: 12}"), Ok(12));
        assert!(id_of("{id: \"12\"}").is_err());
        assert!(id_of("{line: \"x\"}").is_err());
    }
}
