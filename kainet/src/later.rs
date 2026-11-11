//! A call to the `fetch` service that does not wait for the network (P5 task
//! 13): for a caller that cannot block for a request's 30 s, such as comphost,
//! which runs an app on its one thread. Two short calls in place of one long
//! one:
//!
//! - `start <id> <ask>`, with the `NetDest` moved in as for a plain request
//!   ([`crate::wire::Ask`]): `fetch` answers [`STARTED`] at once, then makes
//!   the request and keeps its answer for the caller;
//! - `take <id>`: that answer, as a plain request's (and its body's `Shm`),
//!   once; or [`NONE`] when `fetch` holds none for that caller and id.
//!
//! `fetch` serves one call at a time, so a call that comes while it makes a
//! request waits in the kernel. The caller gives each call a short timeout:
//! one that times out while still waiting is withdrawn, its handle closed
//! with it, and the caller tries again on a later pass. The id is the
//! caller's own, so an answer kept for a request the caller gave up on is
//! never taken for the next.

use crate::fetch::{Error, why};
use crate::wire::{self, Ask, MAX_ASK};
use alloc::format;
use alloc::string::String;

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
