//! Where an ask is answered (ADR 0005 §7, P3 task 7): on `desktop`'s ask card
//! while `desktop` shows and its trusted layer is alive, otherwise on the
//! console. The kernel picks, and an ask has exactly one answering surface at a
//! time: the terminal's thread posts the ask here and waits; `desktop` takes it
//! with ASK_READ and answers it with ASK_ANSWER; if `desktop` stops showing or
//! stops reading before an answer, the ask is withdrawn under the same lock an
//! answer takes, and only then goes to the console. An answer must carry the
//! `ONCE` digest of the call it was shown (`termcore::once::digest_call`), so
//! a card that drew another call cannot answer. The kernel mints the `ONCE` as
//! for the console's menu (`crate::allow`); `desktop` never does.

use crate::{hpet, log};
use alloc::string::String;
use spin::Mutex;
use termcore::menu::Answer;
use x86_64::instructions::interrupts::without_interrupts;

/// The trusted layer is alive if it read the box this recently.
const ALIVE_NS: u64 = 1_000_000_000;
/// How long a card waits for its answer: the card's 2:00 countdown.
pub const WAIT_NS: u64 = 120_000_000_000;

struct Posted {
    id: u64,
    call: String,
    question: String,
    digest: [u8; 32],
    answer: Option<Answer>,
    /// `desktop` has read it: the card shows it.
    taken: bool,
}

struct BoxState {
    posted: Option<Posted>,
    /// HPET time of `desktop`'s last ASK_READ.
    read_ns: u64,
}

static BOX: Mutex<BoxState> = Mutex::new(BoxState {
    posted: None,
    read_ns: 0,
});

fn with<R>(f: impl FnOnce(&mut BoxState) -> R) -> R {
    without_interrupts(|| f(&mut BOX.lock()))
}

/// Whether an unanswered ask is posted and `desktop` has taken it to its card.
pub fn shown() -> bool {
    with(|b| {
        b.posted
            .as_ref()
            .is_some_and(|p| p.taken && p.answer.is_none())
    })
}

/// Whether `desktop`'s trusted layer read the box lately.
pub fn alive() -> bool {
    let read = with(|b| b.read_ns);
    read != 0 && hpet::nanos().saturating_sub(read) < ALIVE_NS
}

/// Posts ask `id` for the card. False if one is posted already.
pub fn post(id: u64, call: &str, question: &str) -> bool {
    with(|b| {
        if b.posted.is_some() {
            return false;
        }
        b.posted = Some(Posted {
            id,
            call: call.into(),
            question: question.into(),
            digest: termcore::once::digest_call(id, call),
            answer: None,
            taken: false,
        });
        true
    })
}

/// What became of the posted ask: its answer, taken out of the box; or, when
/// `withdraw` is true and none came, the ask taken back (None), after which
/// no ASK_ANSWER can answer it.
pub fn finish(withdraw: bool) -> Option<Option<Answer>> {
    with(|b| {
        let answered = b.posted.as_ref().and_then(|p| p.answer);
        match (answered, withdraw) {
            (Some(answer), _) => {
                b.posted = None;
                Some(Some(answer))
            }
            (None, true) => {
                b.posted = None;
                Some(None)
            }
            (None, false) => None,
        }
    })
}

/// ASK_READ: the posted ask as `id`, `call` and `question`, each on its own
/// line, into `out`; empty when none waits or it is answered. Marks the
/// trusted layer alive.
pub fn read(out: &mut String) {
    with(|b| {
        b.read_ns = hpet::nanos().max(1);
        if let Some(p) = b.posted.as_mut().filter(|p| p.answer.is_none()) {
            p.taken = true;
            out.push_str(&alloc::format!("{}\n{}\n{}", p.id, p.call, p.question));
        }
    })
}

/// ASK_ANSWER: answers ask `id`, `yes` for Run and no for Don't, if it is the
/// one posted, unanswered, and `digest` is its call's. Returns whether it took.
pub fn answer(id: u64, yes: bool, digest: &[u8; 32]) -> bool {
    let took = with(|b| match b.posted.as_mut() {
        Some(p) if p.id == id && p.answer.is_none() && p.digest == *digest => {
            p.answer = Some(if yes { Answer::Allow } else { Answer::Deny });
            true
        }
        _ => false,
    });
    if !took {
        log!("ask: an answer to {id} was refused: not the ask posted, or not its call");
    }
    took
}

/// Clears the box when `desktop` exits, so the next one starts unread.
pub fn on_desktop_exit() {
    with(|b| b.read_ns = 0);
}
