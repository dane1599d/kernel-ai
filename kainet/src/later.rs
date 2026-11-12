//! A call to the `fetch` service that does not wait for the network (P5 task
//! 13): for a caller that cannot block for a request's 30 s, such as comphost,
//! which runs an app on its one thread. Two short calls in place of one long
//! one:
//!
//! - `start <id> <ask>`, with the `NetDest` moved in as for a plain request
//!   ([`crate::wire::Ask`]): `fetch` answers [`STARTED`] at once, then makes
//!   the request and keeps its answer for the caller;
//! - `take <id>`: that answer, as a plain request's (and its body's `Shm`),
//!   once; [`GONE`] when `fetch` let it go untaken ([`Keep`]: too many
//!   callers' answers were kept, or it was kept too long), so the caller can
//!   say so at once; or [`NONE`] when it holds none for that caller and id,
//!   not yet made. A take whose reply was lost to the caller's timeout after
//!   `fetch` took it has used that answer (or that `gone`), so the caller's
//!   next take is [`NONE`] until its own limit: a narrow case, told late.
//!
//! `fetch` serves one call at a time, so a call that comes while it makes a
//! request waits in the kernel. The caller gives each call a short timeout:
//! one that times out while still waiting is withdrawn, its handle closed
//! with it, and the caller tries again on a later pass. The id is the
//! caller's own, so an answer kept for a request the caller gave up on is
//! never taken for the next. A start whose [`STARTED`] reply was lost to that
//! timeout after `fetch` took it is made twice, and only the second answer is
//! kept: harmless for GET and HEAD, the only requests [`Ask`] carries, but a
//! method with side effects would need its own way.

use crate::fetch::{Error, why};
use crate::wire::{self, Ask, MAX_ASK};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// `fetch`'s reply to a start: the request is being made.
pub const STARTED: &str = "started";
/// `fetch`'s reply to a take with no answer kept for it.
pub const NONE: &str = "none";
/// The room a start's `start <id> ` takes before its request line.
pub const START_ROOM: usize = 32;
/// The longest message `fetch` reads: a start of the longest request line.
/// A longer one is refused whole ([`too_long`]), never read cut short.
pub const MAX_MESSAGE: usize = MAX_ASK + START_ROOM;

/// `fetch`'s answer to a message longer than [`MAX_MESSAGE`].
pub fn too_long() -> String {
    wire::answer(&Err(Error::BadUrl(why("the request is too long"))))
}

/// `fetch`'s reply to a take whose answer it let go untaken.
pub const GONE: &str = "gone";

/// Answers kept for takes, every caller together: a caller keeps one.
pub const MAX_KEPT: usize = 8;
/// How long an answer is kept for its take.
pub const KEEP_NS: u64 = 60_000_000_000;
/// How many let-go answers are remembered, every caller together, so their
/// takes are answered [`GONE`]: more than [`MAX_KEPT`], so each eviction of a
/// full store is remembered at least as long as the next round of starts.
pub const MAX_GONE: usize = 32;

/// One kept answer: whose, for which start, and since when.
struct Kept<T> {
    caller: u64,
    id: u32,
    value: T,
    at_ns: u64,
}

/// What a take finds.
#[derive(Debug, PartialEq, Eq)]
pub enum Taken<T> {
    /// The answer, taken.
    Answer(T),
    /// The answer was let go untaken.
    Gone,
    /// Nothing for that caller and id: not made yet, or never started.
    None,
}

/// The answers `fetch` keeps for takes: at most [`MAX_KEPT`], one a caller,
/// each for at most [`KEEP_NS`]. What it lets go is handed back for the caller
/// of these to close, and remembered (the last [`MAX_GONE`]) for [`GONE`].
/// The caller is the kernel's process id for the call, never the request's.
pub struct Keep<T> {
    kept: Vec<Kept<T>>,
    gone: VecDeque<(u64, u32)>,
}

impl<T> Default for Keep<T> {
    fn default() -> Self {
        Self {
            kept: Vec::new(),
            gone: VecDeque::new(),
        }
    }
}

impl<T> Keep<T> {
    fn let_go(&mut self, kept: Kept<T>) -> T {
        if self.gone.len() >= MAX_GONE {
            self.gone.pop_front();
        }
        self.gone.push_back((kept.caller, kept.id));
        kept.value
    }

    /// Lets go of what is older than [`KEEP_NS`], handed back.
    pub fn expire(&mut self, now_ns: u64) -> Vec<T> {
        let (old, kept): (Vec<_>, Vec<_>) = core::mem::take(&mut self.kept)
            .into_iter()
            .partition(|k| now_ns.saturating_sub(k.at_ns) >= KEEP_NS);
        self.kept = kept;
        old.into_iter().map(|k| self.let_go(k)).collect()
    }

    /// Keeps `value` for `caller`'s start `id`, in place of anything that
    /// caller had kept, and in place of the oldest when [`MAX_KEPT`] are kept.
    /// What it lets go is handed back.
    pub fn put(&mut self, caller: u64, id: u32, value: T, now_ns: u64) -> Vec<T> {
        let mut dropped = Vec::new();
        if let Some(at) = self.kept.iter().position(|k| k.caller == caller) {
            let old = self.kept.remove(at);
            dropped.push(self.let_go(old));
        }
        if self.kept.len() >= MAX_KEPT {
            let old = self.kept.remove(0);
            dropped.push(self.let_go(old));
        }
        // A start again with an id once let go is new.
        self.gone.retain(|g| *g != (caller, id));
        self.kept.push(Kept {
            caller,
            id,
            value,
            at_ns: now_ns,
        });
        dropped
    }

    /// The answer kept for `caller`'s start `id`, taken; or whether it was let go.
    pub fn take(&mut self, caller: u64, id: u32) -> Taken<T> {
        if let Some(at) = self
            .kept
            .iter()
            .position(|k| k.caller == caller && k.id == id)
        {
            return Taken::Answer(self.kept.remove(at).value);
        }
        match self.gone.iter().position(|g| *g == (caller, id)) {
            Some(at) => {
                self.gone.remove(at);
                Taken::Gone
            }
            None => Taken::None,
        }
    }
}

/// A start or a take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Start { id: u32, ask: Ask },
    Take { id: u32 },
}

impl Message {
    /// Its line.
    pub fn text(&self) -> String {
        match self {
            Self::Start { id, ask } => format!("start {id} {}", ask.text()),
            Self::Take { id } => format!("take {id}"),
        }
    }

    /// The start or take in `text`; `None` when `text` is neither (a plain
    /// request, or anything else, for [`Ask::parse`] to read).
    pub fn parse(text: &str) -> Option<Result<Self, &'static str>> {
        let (word, rest) = text.split_once(' ')?;
        let id = |s: &str| {
            s.parse::<u32>()
                .ok()
                .filter(|_| !s.starts_with('+'))
                .ok_or("the id is not a number")
        };
        Some(match word {
            "start" => rest
                .split_once(' ')
                .ok_or("the start names no request")
                .and_then(|(n, ask)| {
                    Ok(Self::Start {
                        id: id(n)?,
                        ask: Ask::parse(ask)?,
                    })
                }),
            "take" => id(rest).map(|id| Self::Take { id }),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::Request;
    use crate::http::{Method, Range};
    use crate::wire::MAX_URL;
    use alloc::string::ToString;

    fn ask(url: &str) -> Ask {
        Ask {
            request: Request {
                url: url.into(),
                method: Method::Get,
                range: None,
            },
            hops: 1,
        }
    }

    #[test]
    fn a_start_and_a_take_read_back_the_same() {
        let start = Message::Start {
            id: 7,
            ask: ask("https://updates.kai.test/hello"),
        };
        assert_eq!(
            start.text(),
            "start 7 get https://updates.kai.test/hello hops=1"
        );
        assert_eq!(Message::parse(&start.text()), Some(Ok(start)));
        let take = Message::Take { id: u32::MAX };
        assert_eq!(Message::parse(&take.text()), Some(Ok(take)));
    }

    #[test]
    fn a_kept_answer_is_taken_once_by_its_caller_alone() {
        let mut keep = Keep::default();
        assert!(keep.put(1, 7, "a", 0).is_empty());
        assert_eq!(keep.take(2, 7), Taken::None);
        assert_eq!(keep.take(1, 8), Taken::None);
        assert_eq!(keep.take(1, 7), Taken::Answer("a"));
        assert_eq!(keep.take(1, 7), Taken::None);
    }

    #[test]
    fn an_evicted_answer_is_gone_at_once_not_none() {
        let mut keep = Keep::default();
        for caller in 0..MAX_KEPT as u64 {
            assert!(keep.put(caller, 1, caller, 0).is_empty());
        }
        // A ninth caller evicts the first caller's answer, handed back to close.
        assert_eq!(keep.put(99, 1, 99, 0), [0]);
        assert_eq!(keep.take(0, 1), Taken::Gone);
        // Told once; after that the id is unknown again.
        assert_eq!(keep.take(0, 1), Taken::None);
        assert_eq!(keep.take(1, 1), Taken::Answer(1));
    }

    #[test]
    fn an_expired_or_replaced_answer_is_gone() {
        let mut keep = Keep::default();
        keep.put(1, 1, "old", 0);
        // The same caller's next start replaces it (a lost `started`).
        assert_eq!(keep.put(1, 2, "new", 0), ["old"]);
        assert_eq!(keep.take(1, 1), Taken::Gone);
        keep.put(2, 1, "late", 0);
        assert!(keep.expire(KEEP_NS - 1).is_empty());
        assert_eq!(keep.expire(KEEP_NS), ["new", "late"]);
        assert_eq!(keep.take(1, 2), Taken::Gone);
        assert_eq!(keep.take(2, 1), Taken::Gone);
    }

    #[test]
    fn the_let_go_ids_remembered_are_bounded_and_a_reused_id_is_new() {
        let mut keep = Keep::default();
        for id in 0..(MAX_GONE as u32 + 4) {
            keep.put(1, id, id, 0);
        }
        // Each start replaced the one before: the oldest let-go ids are forgotten.
        assert_eq!(keep.take(1, 0), Taken::None);
        assert_eq!(keep.take(1, 4), Taken::Gone);
        // An id started again is kept, not gone.
        keep.put(1, 5, 500, 0);
        assert_eq!(keep.take(1, 5), Taken::Answer(500));
    }

    #[test]
    fn a_plain_request_is_neither_and_a_bad_one_is_refused_by_name() {
        assert_eq!(Message::parse("get https://a.test/x"), None);
        assert_eq!(Message::parse("start"), None);
        for (text, why) in [
            ("start 1", "the start names no request"),
            ("start x get https://a.test/x", "the id is not a number"),
            ("start +1 get https://a.test/x", "the id is not a number"),
            (
                "start 1 put https://a.test/x",
                "the request is not get or head",
            ),
            ("take ", "the id is not a number"),
            ("take 1 2", "the id is not a number"),
            ("take 4294967296", "the id is not a number"),
        ] {
            assert_eq!(Message::parse(text), Some(Err(why)), "{text:?}");
        }
    }

    #[test]
    fn the_longest_start_fits_a_message_and_a_longer_one_is_refused_by_name() {
        let mut ask = ask(&format!("https://a.test/{}", "x".repeat(MAX_URL - 15)));
        ask.request.range = Some(Range {
            first: u64::MAX - 1,
            last: Some(u64::MAX),
        });
        ask.hops = 5;
        let start = Message::Start { id: u32::MAX, ask };
        let text = start.text();
        assert!(text.len() <= MAX_MESSAGE, "{}", text.len());
        assert_eq!(Message::parse(&text), Some(Ok(start)));
        assert_eq!(too_long(), "err bad-url the request is too long");
        assert_eq!(
            wire::parse_answer(&too_long()),
            Ok(Err(Error::BadUrl("the request is too long".to_string())))
        );
    }
}
