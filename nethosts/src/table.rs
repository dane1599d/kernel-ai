//! The curated host table (ADR 0002 D3): a request word and the host it means.
//! "youtube" is `www.youtube.com`, so "play something on youtube" grounds a fetch
//! from it with no address typed. An installed app adds its own word: "weather" is
//! the host the Weather app declares, so "what's the weather in Leeds" grounds a
//! fetch from `api.weather.example`.
//!
//! The table gives a host only for a word the line holds, and only that host: a
//! word never grounds a lookalike (`youtube.xyz`), a subdomain or a parent.

use crate::host::{Host, HostError, LineWord};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What the registry ships: the entries `agentabi`'s `HOSTS` holds today.
pub const CURATED: &[(&str, &str)] = &[
    ("youtube", "www.youtube.com"),
    ("wikipedia", "en.wikipedia.org"),
];

/// Shortest and longest request word.
pub const MIN_WORD: usize = 3;
pub const MAX_WORD: usize = 16;
/// Most hosts one app may declare for its word.
pub const MAX_APP_HOSTS: usize = 4;
/// Most entries the table holds.
pub const MAX_ENTRIES: usize = 64;

/// Words that name no site: a kind of thing, or a word of every request. An app
/// cannot take one as its word, since a line that says "the file" or "please"
/// would then ground its host.
const RESERVED: &[&str] = &[
    "app",
    "apps",
    "application",
    "check",
    "component",
    "device",
    "fetch",
    "file",
    "files",
    "find",
    "folder",
    "for",
    "from",
    "get",
    "here",
    "into",
    "kai",
    "network",
    "news",
    "open",
    "package",
    "packages",
    "please",
    "process",
    "processes",
    "program",
    "programs",
    "search",
    "server",
    "service",
    "services",
    "settings",
    "show",
    "site",
    "system",
    "that",
    "the",
    "this",
    "web",
    "website",
    "what",
    "whats",
    "with",
    "your",
];

/// Where an entry came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Shipped with the registry.
    Curated,
    /// Declared by this installed app.
    App(String),
}

/// One word and the host it means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub word: String,
    pub host: Host,
    pub by: Provider,
}

/// Why an app's word was not added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableError {
    /// The app's name is not one plain word, or is one that names no site.
    NotAWord,
    /// The app declares no host, or more than [`MAX_APP_HOSTS`].
    Hosts,
    /// One of its hosts is not a host.
    BadHost(HostError),
    /// A curated entry has the word: an app cannot shadow it.
    Shadows,
    /// Another app has the word already (its name).
    Taken(String),
    /// The table is full.
    Full,
}

impl TableError {
    /// In words for the log.
    pub fn words(&self) -> String {
        match self {
            Self::NotAWord => String::from("its name is not one plain word that names a site"),
            Self::Hosts => String::from("it declares no host, or too many"),
            Self::BadHost(why) => alloc::format!("a host it declares is not one: {}", why.words()),
            Self::Shadows => String::from("a word the host table ships is not an app's to take"),
            Self::Taken(app) => alloc::format!("{app} has that word"),
            Self::Full => String::from("the host table is full"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    entries: Vec<Entry>,
}

/// An app's name as a request word: lowercase letters and digits, starting with a
/// letter, [`MIN_WORD`] to [`MAX_WORD`] of them, and not a reserved word.
fn word_ok(word: &str) -> bool {
    let plain = word.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && word
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    plain && (MIN_WORD..=MAX_WORD).contains(&word.len()) && !RESERVED.contains(&word)
}

impl Table {
    /// The table the registry ships.
    pub fn curated() -> Self {
        let entries = CURATED
            .iter()
            .filter_map(|(word, host)| {
                Some(Entry {
                    word: (*word).to_string(),
                    host: Host::parse(host).ok()?,
                    by: Provider::Curated,
                })
            })
            .collect();
        Self { entries }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// An installed app's word, `app` (its package name, from the signed
    /// manifest), and the hosts it declares. All or nothing: a word a curated
    /// entry or another app holds is refused, so no app shadows a site or shares a
    /// word, and a bad host refuses the lot.
    pub fn add_app(&mut self, app: &str, hosts: &[&str]) -> Result<(), TableError> {
        if !word_ok(app) {
            return Err(TableError::NotAWord);
        }
        if hosts.is_empty() || hosts.len() > MAX_APP_HOSTS {
            return Err(TableError::Hosts);
        }
        let mut parsed: Vec<Host> = Vec::new();
        for text in hosts {
            let host = Host::parse(text).map_err(TableError::BadHost)?;
            if !parsed.contains(&host) {
                parsed.push(host);
            }
        }
        if let Some(entry) = self.entries.iter().find(|e| e.word == app) {
            return Err(match &entry.by {
                Provider::Curated => TableError::Shadows,
                Provider::App(other) => TableError::Taken(other.clone()),
            });
        }
        if self.entries.len() + parsed.len() > MAX_ENTRIES {
            return Err(TableError::Full);
        }
        for host in parsed {
            self.entries.push(Entry {
                word: app.to_string(),
                host,
                by: Provider::App(app.to_string()),
            });
        }
        Ok(())
    }

    /// The app is removed: its word and hosts go, so its host stops being
    /// grounded by a word. How many entries went.
    pub fn remove_app(&mut self, app: &str) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !matches!(&entry.by, Provider::App(name) if name == app));
        before - self.entries.len()
    }

    /// The words that mean `host`, each with whether the line holds it counting
    /// (`Some(true)`), only in a negated clause (`Some(false)`), or not at all.
    pub(crate) fn words_in<'a>(
        &'a self,
        line: &[LineWord],
        host: &Host,
    ) -> impl Iterator<Item = (&'a str, Option<bool>)> {
        self.entries
            .iter()
            .filter(move |entry| entry.host == *host)
            .map(|entry| {
                let holds = line.iter().filter(|w| w.text == entry.word);
                (entry.word.as_str(), counted(holds))
            })
    }
}

/// Whether a word the line holds counts: `Some(true)` if any place of it does,
/// `Some(false)` if it is there only in negated clauses, `None` if it is not there.
pub(crate) fn counted<'a>(mut holds: impl Iterator<Item = &'a LineWord>) -> Option<bool> {
    let mut found = None;
    for word in holds.by_ref() {
        if word.counts {
            return Some(true);
        }
        found = Some(false);
    }
    found
}
