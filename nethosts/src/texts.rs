//! What the person reads about a fetch: the ask when its host is not grounded, the
//! note on why one ran, and the result line. These are words for the person, not
//! model input, so they change no prompt and need no fluency run. A host is shown
//! as the code normalised it, never as the model wrote it, so what the person
//! reads is what the OS would connect to.

use crate::ground::{Ground, Ungrounded};
use crate::host::Host;
use alloc::format;
use alloc::string::String;

/// Longest text of a host the person is shown that was not a host.
const SHOWN: usize = 64;

/// `text` made safe to show: printable ASCII (anything else becomes `?`) and cut
/// short, for a host that was not one.
fn shown(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .take(SHOWN)
        .collect();
    if text.chars().count() > SHOWN {
        out.push_str("...");
    }
    out
}

impl Ground {
    /// Why the fetch ran: `you typed it`, `"weather" is in the host table`.
    pub fn note(&self) -> String {
        match self {
            Self::Typed => String::from("you typed it"),
            Self::Table(word) => format!("\"{word}\" is in the host table"),
            Self::Granted => String::from("you allowed it before"),
            Self::Source(what) => format!("{what}; what it sends is checked before use"),
        }
    }
}

impl Ungrounded {
    /// Why it asks, as a clause: the same words the shell gives (`the request did
    /// not name that site`).
    pub fn why(&self) -> String {
        match self {
            Self::NotNamed => String::from("the request did not name that site"),
            Self::FromContent => String::from("a page or file gave that address, not your request"),
            Self::Negated => String::from("the request says not to use that site"),
            Self::BadHost(why) => format!("that is not a plain host name: {}", why.words()),
        }
    }

    /// The question, naming the host: `Fetch from evil.example? The request did
    /// not name that site.` `raw` is what the model wrote; a name that parses is
    /// shown in its normal form, an encoded international one with a warning.
    pub fn ask(&self, raw: &str) -> String {
        let (name, warn) = match Host::parse(raw) {
            Ok(host) if host.is_punycode() => (
                String::from(host.as_str()),
                " It is an encoded international name, which can look like another site.",
            ),
            Ok(host) => (String::from(host.as_str()), ""),
            Err(_) => (shown(raw), ""),
        };
        let why = self.why();
        let mut chars = why.chars();
        let first = chars.next().map(|c| c.to_ascii_uppercase());
        let why: String = first.into_iter().chain(chars).collect();
        format!("Fetch from {name}? {why}.{warn}")
    }
}

/// The result line of a fetch that ran: `Fetched from www.youtube.com ("youtube"
/// is in the host table).`
pub fn fetched(host: &Host, ground: &Ground) -> String {
    format!("Fetched from {host} ({}).", ground.note())
}
