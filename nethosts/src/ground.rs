//! The grounding rule (ADR 0002 D3): a fetch from a grounded host runs, and one
//! from any other asks.
//!
//! A host is grounded only if
//! 1. the user typed it (the line names it, as an address or a bare name);
//! 2. it is the entry for a request word the line holds, in the curated host
//!    table or an installed app's word;
//! 3. the user said yes to this exact host before (a grant record, ADR 0001); or
//! 4. it is on the OS's own source list and the bytes fetched from it are checked
//!    against signed metadata before use.
//!
//! A label match is not enough: only the exact host. A host that a page or a file
//! gave, that the model did not take from the line, is never grounded, whatever
//! else is true of it, and nor is a host that is not plainly one.

use crate::host::{Host, HostError, LineWord, host_words, typed_hosts, typed_spans};
use crate::table::{Table, counted};
use alloc::string::String;
use alloc::vec::Vec;

/// The user's yes to a host before, as a grant record holds it (ADR 0001). The
/// record is for one exact host: a grant for `api.weather.example` is not one for
/// `x.api.weather.example`.
pub trait Granted {
    fn granted(&self, host: &Host) -> bool;

    /// The apps whose grants ground `host`, by package name: their words in
    /// the line ask for that app's site. None by default.
    fn holders(&self, _host: &Host) -> Vec<String> {
        Vec::new()
    }
}

/// No grants: nothing was said yes to.
pub struct NoGrants;

impl Granted for NoGrants {
    fn granted(&self, _host: &Host) -> bool {
        false
    }
}

/// A list of granted hosts, for tests and for a runner that reads its records
/// into one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    hosts: Vec<Host>,
}

impl Grants {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(&mut self, host: Host) {
        if !self.hosts.contains(&host) {
            self.hosts.push(host);
        }
    }

    /// Takes a grant back (Settings' revoke, or `undo`): the host is no longer
    /// grounded by it.
    pub fn revoke(&mut self, host: &Host) {
        self.hosts.retain(|granted| granted != host);
    }
}

impl Granted for Grants {
    fn granted(&self, host: &Host) -> bool {
        self.hosts.contains(host)
    }
}

/// A host on the OS's own source list: the signed repository's, the pinned model
/// host's. `what` says whose, for the person ("the signed repository").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub host: Host,
    pub what: &'static str,
}

/// Where the host in a call came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The model wrote it as an argument, from the user's line and the registry.
    Line,
    /// Read content (a page, a file, an app's answer, the State line) gave it.
    Content,
}

/// What is done with the bytes fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bytes {
    /// Used as they came: a page the model reads.
    Unchecked,
    /// Checked against signed metadata before any use: a package, a model blob.
    Checked,
}

/// Why a host is grounded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ground {
    /// The line names it.
    Typed,
    /// A request word of the line means it: this word.
    Table(String),
    /// The user said yes to it before.
    Granted,
    /// The OS's own source list, whose bytes are checked: whose.
    Source(&'static str),
}

impl Ground {
    /// Whether the user's line itself names the destination: typed, or a word of
    /// it that the table gives. This is what P-F means by a destination the line
    /// names, for a payload of read content: a grant or the OS's list is not the
    /// user saying where this data goes.
    pub fn names_destination(&self) -> bool {
        matches!(self, Self::Typed | Self::Table(_))
    }
}

/// Why a host is not grounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ungrounded {
    /// Nothing the user said, or said yes to, names it.
    NotNamed,
    /// Read content gave it, not the line.
    FromContent,
    /// The line names it only in a clause that says not to ("don't use weather"),
    /// so it grounds nothing, a grant included.
    Negated,
    /// It is not a plain host name.
    BadHost(HostError),
}

/// The tier of a fetch from the host: run, or ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Run(Ground),
    Ask(Ungrounded),
}

/// The data the rule reads.
pub struct Grounding<'a> {
    pub table: &'a Table,
    pub granted: &'a dyn Granted,
    pub sources: &'a [Source],
}

impl Grounding<'_> {
    /// Whether a fetch from `host`, which the model wrote as a call argument
    /// (`origin` says from where), runs for the request `line`.
    ///
    /// `words` are the line's words as the agent core's consent reads them, each
    /// marked as counting or not: a word in a clause that negates ("don't use
    /// weather", "never fetch youtube.com") does not count, so it grounds nothing,
    /// and a host the line names only there asks even if the user allowed it
    /// before: the line says no. Whether a question clause counts is the caller's
    /// word to mark; a read is naturally asked as a question ("what's the weather
    /// in Leeds?"), so the P5 plan's example needs it to.
    pub fn decide(
        &self,
        line: &str,
        words: &[LineWord],
        host: &str,
        origin: Origin,
        bytes: Bytes,
    ) -> Verdict {
        let host = match Host::parse(host) {
            Ok(host) => host,
            Err(why) => return Verdict::Ask(Ungrounded::BadHost(why)),
        };
        if origin == Origin::Content {
            return Verdict::Ask(Ungrounded::FromContent);
        }
        // A typed host counts when every word it is written with does. Typed but
        // not counting means a word of it sits in a clause that negates (or the
        // caller left it out): the line names it only to say no.
        // Each place the line writes the host is judged on its own words: a
        // host named only in a clause that says no is not grounded because the
        // same labels count in another host ("never fetch example.com, fetch
        // www.example.com instead"). Words that are not the line's own (a
        // caller's partial list) fall back to counting a word anywhere.
        let typed = typed_hosts(line).contains(&host);
        let own = host_words(&host);
        let anywhere = || {
            own.iter()
                .all(|text| counted(words.iter().filter(|w| w.text == *text)) == Some(true))
        };
        let counts = match typed_spans(line, &host) {
            spans if spans.is_empty() => anywhere(),
            spans => spans.into_iter().any(|span| match words.get(span.clone()) {
                Some(here) if here.iter().zip(&own).all(|(w, text)| w.text == *text) => {
                    here.iter().all(|w| w.counts)
                }
                _ => anywhere(),
            }),
        };
        if typed && counts {
            return Verdict::Run(Ground::Typed);
        }
        let mut barred = typed;
        for (word, held) in self.table.words_in(words, &host) {
            match held {
                Some(true) => return Verdict::Run(Ground::Table(String::from(word))),
                Some(false) => barred = true,
                None => {}
            }
        }
        if barred {
            return Verdict::Ask(Ungrounded::Negated);
        }
        if self.granted.granted(&host) {
            return Verdict::Run(Ground::Granted);
        }
        if bytes == Bytes::Checked
            && let Some(source) = self.sources.iter().find(|source| source.host == host)
        {
            return Verdict::Run(Ground::Source(source.what));
        }
        Verdict::Ask(Ungrounded::NotNamed)
    }
}
