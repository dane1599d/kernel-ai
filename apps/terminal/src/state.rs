//! What the terminal keeps through a restart or a live edit (`kai-app` state,
//! CBOR, at most 64 KiB): the scrollback, the numbering, the shell's prompt and
//! where the next line goes. Read back, it is checked as input from outside:
//! limits are applied again and anything unknown is dropped.

use crate::span::{Kind, MAX_SPANS, Span, fit};
use crate::{Ask, DEFAULT_PROMPT, Phase, Terminal, valid_prompt};
use alloc::string::String;
use alloc::vec::Vec;
use minicbor::{Decode, Encode};

/// The state schema.
pub const SCHEMA: u32 = 1;

const READY: u8 = 0;
const CONTINUING: u8 = 1;
const WAITING: u8 = 2;
const ASKING_CONFIRM: u8 = 3;
const ASKING_MENU: u8 = 4;

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct State {
    #[n(0)]
    pub spans: Vec<Span>,
    #[n(1)]
    pub next_seq: u32,
    #[n(2)]
    pub prompt: String,
    #[n(3)]
    pub phase: u8,
    /// The question's id while asking.
    #[n(4)]
    pub ask: u64,
}

impl Terminal {
    pub fn to_state(&self) -> State {
        let (phase, ask) = match self.phase {
            Phase::Ready => (READY, 0),
            Phase::Continuing => (CONTINUING, 0),
            Phase::Waiting => (WAITING, 0),
            Phase::Asking(Ask { id, menu: false }) => (ASKING_CONFIRM, id),
            Phase::Asking(Ask { id, menu: true }) => (ASKING_MENU, id),
        };
        State {
            spans: self.spans.clone(),
            next_seq: self.next_seq,
            prompt: self.base.clone(),
            phase,
            ask,
        }
    }

    /// The terminal `state` describes, with its limits applied again. A line that
    /// was with the shell when the app stopped has no answer coming, so the
    /// prompt returns and says so.
    pub fn from_state(state: State) -> Self {
        let mut spans: Vec<Span> = state
            .spans
            .into_iter()
            .map(|s| Span {
                prompt: s.prompt.chars().take(crate::MAX_PROMPT).collect(),
                text: fit(&s.text),
                ..s
            })
            .collect();
        let excess = spans.len().saturating_sub(MAX_SPANS);
        spans.drain(..excess);
        // Rows are numbered one after another, so the window can give each its
        // own node; a state that says otherwise is numbered again from its first.
        let first = spans.first().map_or(1, |s| s.seq.max(1));
        for (i, span) in spans.iter_mut().enumerate() {
            span.seq = first.wrapping_add(i as u32).max(1);
        }
        let last = spans.last().map_or(0, |s| s.seq);
        let base = match valid_prompt(&state.prompt) {
            true => state.prompt,
            false => String::from(DEFAULT_PROMPT),
        };
        let phase = match state.phase {
            CONTINUING => Phase::Continuing,
            ASKING_CONFIRM => Phase::Asking(Ask {
                id: state.ask,
                menu: false,
            }),
            ASKING_MENU => Phase::Asking(Ask {
                id: state.ask,
                menu: true,
            }),
            _ => Phase::Ready,
        };
        let mut t = Self {
            spans,
            next_seq: state.next_seq.max(last.wrapping_add(1)).max(1),
            base,
            phase,
        };
        if state.phase == WAITING {
            t.push(
                Kind::Notice,
                "",
                "the terminal restarted while a line was running",
                false,
            );
        }
        t
    }
}
