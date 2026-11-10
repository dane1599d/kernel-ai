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

use crate::wire::Ask;
use alloc::format;
use alloc::string::String;

/// `fetch`'s reply to a start: the request is being made.
pub const STARTED: &str = "started";
/// `fetch`'s reply to a take with no answer kept for it.
pub const NONE: &str = "none";

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
    use crate::http::Method;

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
}
