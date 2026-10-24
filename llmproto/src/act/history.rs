//! Conversation memory for `act` (P1 plan section 4): the turns before the
//! current one, written by code, never by the model. Each holds the user's
//! line, the call the model emitted exactly as decoded and the result line the
//! shell wrote (`Done: ... (was ...)`, `Taken as ...`). The State line is never
//! kept, since it changes every request and would spoil the cache.
//!
//! History sits after the `act` MODE turn, so a new request prefills only its
//! own turn. It is context, never consent: consent reads only the current line
//! (ADR 0002 D3), and nothing here returns from history but its text in the
//! prompt. It lives in RAM until reboot, a model switch or `new`.
//!
//! It holds at most [`budget`] tokens; when a new turn would go over, the
//! oldest half is dropped (the llm service then re-prefills the rest while
//! idle).

use crate::chat::{Message, Role};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// History's ceiling (P1 plan section 4).
pub const HISTORY_MAX: usize = 512;
/// What the rest of the context keeps (P1 plan section 4): the chat template's
/// turns, the State line, the request and the answer reserve.
pub const TEMPLATE_TOKENS: usize = 32;
pub const STATE_TOKENS: usize = 64;
pub const REQUEST_TOKENS: usize = 160;
pub const ANSWER_TOKENS: usize = 1024;
/// Under this, the plan's index level would collapse app groups (ADR 0010 §6);
/// P1's pack leaves the full 512.
pub const HISTORY_FLOOR: usize = 192;
/// Longest result line kept, in bytes.
pub const RESULT_MAX_BYTES: usize = 300;
/// Longest call kept, in bytes: a script's call can be 3 KiB.
pub const CALL_MAX_BYTES: usize = 4096;

/// `HISTORY_BUDGET = min(512, context − pack − 32 − 64 − 160 − 1,024)`, with
/// `pack` the pack and its MODE turn in tokens.
pub fn budget(context: usize, pack: usize) -> usize {
    let rest = TEMPLATE_TOKENS + STATE_TOKENS + REQUEST_TOKENS + ANSWER_TOKENS;
    context.saturating_sub(pack + rest).min(HISTORY_MAX)
}

/// One earlier turn, as code wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub line: String,
    pub call: String,
    pub result: String,
}

impl Turn {
    /// Its three messages: the request, the call, the result.
    pub fn messages(&self) -> [Message; 3] {
        [
            Message::new(Role::User, &format!("Request: {}", self.line)),
            Message::new(Role::Assistant, &self.call),
            Message::new(Role::User, &format!("Result: {}", self.result)),
        ]
    }
}

/// The turns kept, oldest first, with what each costs in tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    turns: VecDeque<(Turn, usize)>,
}

impl History {
    pub const fn new() -> Self {
        Self {
            turns: VecDeque::new(),
        }
    }

    pub fn turns(&self) -> impl Iterator<Item = &Turn> {
        self.turns.iter().map(|(turn, _)| turn)
    }

    pub fn len(&self) -> usize {
        self.turns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Tokens the kept turns take.
    pub fn tokens(&self) -> usize {
        self.turns.iter().map(|(_, cost)| cost).sum()
    }

    /// `new`: forget everything.
    pub fn clear(&mut self) {
        self.turns.clear();
    }

    /// Keeps `turn`, which takes `cost` tokens, within `budget`: while it would
    /// go over, the oldest half goes. A turn over the budget on its own is not
    /// kept, and nothing older is dropped for it. Returns whether turns were
    /// dropped, so the caller re-prefills.
    pub fn push(&mut self, turn: Turn, cost: usize, budget: usize) -> bool {
        if cost > budget {
            return false;
        }
        let mut dropped = false;
        while self.tokens() + cost > budget {
            let half = self.turns.len().div_ceil(2);
            self.turns.drain(..half);
            dropped = true;
        }
        self.turns.push_back((turn, cost));
        dropped
    }

    /// Every kept turn's messages, oldest first.
    pub fn messages(&self) -> Vec<Message> {
        self.turns().flat_map(Turn::messages).collect()
    }
}

/// The shell's `remember <line len> <call len> <result len>`, a newline, then
/// the three texts: the turn just finished, sent once its result is on screen.
pub fn encode_remember(turn: &Turn) -> String {
    format!(
        "remember {} {} {}\n{}{}{}",
        turn.line.len(),
        turn.call.len(),
        turn.result.len(),
        turn.line,
        turn.call,
        turn.result
    )
}

pub fn decode_remember(text: &str) -> Option<Turn> {
    let (header, body) = text.split_once('\n')?;
    let mut words = header.strip_prefix("remember ")?.split(' ');
    let mut number = || words.next()?.parse::<usize>().ok();
    let (line, call, result) = (number()?, number()?, number()?);
    if body.len() != line.checked_add(call)?.checked_add(result)?
        || line > super::LINE_MAX_BYTES
        || call > CALL_MAX_BYTES
        || result > RESULT_MAX_BYTES
    {
        return None;
    }
    let turn = Turn {
        line: body.get(..line)?.to_string(),
        call: body.get(line..line + call)?.to_string(),
        result: body.get(line + call..)?.to_string(),
    };
    // The call must be one the decoder could have written.
    super::parse(&turn.call).ok()?;
    Some(turn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(n: usize) -> Turn {
        Turn {
            line: format!("switch {n}"),
            call: String::from(r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#),
            result: String::from("Done: policy round-robin (was interactive). undo"),
        }
    }

    #[test]
    fn the_budget_follows_the_plan() {
        // P1's pack and MODE: 878 real tokens (#48's token count).
        assert_eq!(budget(3072, 878), HISTORY_MAX);
        assert_eq!(budget(3072, 1_400), 3072 - 1_400 - 1_280);
        assert_eq!(budget(2048, 1_000), 0);
        // The pack's budget leaves history its full 512 tokens, over the floor.
        let pack = crate::pack::PACK_BUDGET;
        assert!(budget(kabi::model::CONTEXT_CAP, pack) >= HISTORY_FLOOR);
        assert_eq!(budget(kabi::model::CONTEXT_CAP, 878), 512);
    }

    #[test]
    fn history_stays_within_its_budget_dropping_the_oldest_half() {
        let mut history = History::new();
        for n in 0..8 {
            assert!(!history.push(turn(n), 60, 512));
        }
        assert_eq!(history.tokens(), 480);
        // The ninth goes over: the oldest four go.
        assert!(history.push(turn(8), 60, 512));
        assert_eq!(history.len(), 5);
        assert_eq!(history.turns().next().unwrap().line, "switch 4");
        assert!(history.tokens() <= 512);
        // A turn bigger than the whole budget is not kept and drops nothing.
        assert!(!history.push(turn(9), 600, 512));
        assert_eq!(history.len(), 5);
        history.clear();
        assert!(history.is_empty() && history.tokens() == 0);
        // With no room at all nothing is kept.
        assert!(!history.push(turn(1), 10, 0));
        assert!(history.is_empty());
    }

    #[test]
    fn a_turn_is_its_request_call_and_result_never_the_state_line() {
        let messages = turn(1).messages();
        assert_eq!(messages[0].content, "Request: switch 1");
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
        assert_eq!(
            messages[2].content,
            "Result: Done: policy round-robin (was interactive). undo"
        );
        assert!(messages.iter().all(|m| !m.content.contains("State:")));
    }

    #[test]
    fn remember_round_trips_and_refuses_what_the_decoder_could_not_write() {
        let t = turn(3);
        assert_eq!(decode_remember(&encode_remember(&t)), Some(t.clone()));
        let not_a_call = Turn {
            call: String::from("rm -rf /"),
            ..t.clone()
        };
        assert_eq!(decode_remember(&encode_remember(&not_a_call)), None);
        let long = Turn {
            result: "x".repeat(RESULT_MAX_BYTES + 1),
            ..t
        };
        assert_eq!(decode_remember(&encode_remember(&long)), None);
        for bad in [
            "",
            "remember",
            "remember 1 2\nabc",
            "remember 1 1 1\nab",
            "act 1 0 0\n",
        ] {
            assert_eq!(decode_remember(bad), None, "{bad:?}");
        }
    }
}
