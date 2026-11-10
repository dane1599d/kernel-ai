//! A test fixture for the `net-grant` boot case (P5 task 13), never shipped.
//! [`Fixture`] has no window: when it starts it asks `kai:net` for the first
//! of [`URLS`], and each time an answer comes it logs one line saying what the
//! request came to ([`outcome_line`]) and asks for the next. The case answers
//! the shell's cards for the hosts as they come up, so the lines show a host
//! allowed, a redirect to another host asked about by its own name, a host
//! denied, one outside the app's ceiling never asked about, and the first
//! host again with no card.
//!
//! Only test images carry it (`xtask::components::TEST_ONLY`).

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use compsdk::{Component, ImportError};

#[cfg(target_family = "wasm")]
mod bind;

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// What it asks for, in order: a host it is allowed; one whose download
/// redirects to another host; one it is denied; one its ceiling leaves out;
/// the first again, granted already.
pub const URLS: &[&str] = &[
    "https://updates.kai.test/hello",
    "https://releases.kai.test/dl",
    "https://elsewhere.kai.test/hello",
    "https://other.example/hello",
    "https://updates.kai.test/hello",
];
/// The most body bytes logged.
const MAX_SHOWN: usize = 80;

/// What a request came to, as the fixture logs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Response {
        status: u16,
        url: String,
        body: Vec<u8>,
    },
    /// The error's name, and its text if it has one.
    Error(String),
}

/// `text` as one log line's worth: printable, at most [`MAX_SHOWN`] bytes,
/// trailing space dropped.
fn shown(bytes: &[u8]) -> String {
    let text: String = bytes
        .iter()
        .take(MAX_SHOWN)
        .map(|&b| match b {
            b' ' => ' ',
            b if b.is_ascii_graphic() => b as char,
            _ => ' ',
        })
        .collect();
    String::from(text.trim_end())
}

/// The line logged for the request for `url`.
pub fn outcome_line(url: &str, outcome: &Outcome) -> String {
    match outcome {
        Outcome::Response {
            status,
            url: from,
            body,
        } => format!("net-fixture: {url}: {status} from {from}: {}", shown(body)),
        Outcome::Error(error) => format!("net-fixture: {url}: {}", shown(error.as_bytes())),
    }
}

pub struct Fixture {
    /// The index in [`URLS`] of the request on its way.
    at: usize,
    /// Its ticket.
    waiting: Option<u32>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            at: 0,
            waiting: None,
        }
    }

    /// Asks for the URL at `at`, or says it is done.
    fn ask(&mut self) {
        match URLS.get(self.at) {
            Some(url) => self.waiting = Some(fetch(url)),
            None => {
                self.waiting = None;
                log("net-fixture: done");
            }
        }
    }

    /// The answer to `ticket`: logged, and the next URL asked for.
    pub fn answered(&mut self, ticket: u32, outcome: Outcome) {
        if self.waiting != Some(ticket) {
            log(&format!(
                "net-fixture: an answer for ticket {ticket}, not asked"
            ));
            return;
        }
        if let Some(url) = URLS.get(self.at) {
            log(&outcome_line(url, &outcome));
        }
        self.at += 1;
        self.ask();
    }
}

fn log(text: &str) {
    #[cfg(target_family = "wasm")]
    compsdk::log(text);
    #[cfg(not(target_family = "wasm"))]
    let _ = text;
}

#[cfg(target_family = "wasm")]
fn fetch(url: &str) -> u32 {
    bind::fetch(url)
}

/// Off wasm there is no host to ask: every request is ticket 0.
#[cfg(not(target_family = "wasm"))]
fn fetch(_url: &str) -> u32 {
    0
}

impl Component for Fixture {
    const SCHEMA: u32 = SCHEMA;

    /// Asks for the first URL at once.
    fn init() -> Self {
        let mut app = Self::new();
        app.ask();
        app
    }

    /// A new version starts the list again.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        compsdk::state::decode::<()>(state, SCHEMA).ok_or(ImportError::Bad)?;
        Ok(Self::init())
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| compsdk::state::encode(SCHEMA, &()))
    }

    fn health(&self) -> u32 {
        0
    }

    fn self_test(&self) -> u32 {
        0
    }
}

compsdk::component!(Fixture);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_says_the_status_where_it_came_from_and_the_body() {
        let ok = Outcome::Response {
            status: 200,
            url: String::from("https://assets.kai.test/hello"),
            body: b"kai mock: hello\n".to_vec(),
        };
        assert_eq!(
            outcome_line(URLS[1], &ok),
            "net-fixture: https://releases.kai.test/dl: 200 from https://assets.kai.test/hello: kai mock: hello"
        );
        let refused = Outcome::Error(String::from("refused elsewhere.kai.test"));
        assert_eq!(
            outcome_line(URLS[2], &refused),
            "net-fixture: https://elsewhere.kai.test/hello: refused elsewhere.kai.test"
        );
        let long = Outcome::Error(format!("http {}\n", "x".repeat(500)));
        assert!(outcome_line(URLS[0], &long).len() < 140);
    }

    #[test]
    fn each_answer_moves_on_to_the_next_url() {
        let mut app = Fixture::init();
        assert_eq!((app.at, app.waiting), (0, Some(0)));
        // An answer for another ticket is not this request's.
        app.answered(5, Outcome::Error(String::from("timeout")));
        assert_eq!(app.at, 0);
        for at in 1..=URLS.len() {
            app.answered(0, Outcome::Error(String::from("timeout")));
            assert_eq!(app.at, at);
        }
        assert_eq!(app.waiting, None, "done");
    }
}
