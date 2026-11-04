//! A host name in one normal form, and the hosts a request names.
//!
//! Everything that compares hosts compares these, so `WWW.YouTube.com.` and
//! `www.youtube.com` are one host and `www.youtube.com.evil.example` is another.
//! Nothing here guesses: a name that is not plainly ASCII letters, digits and
//! hyphens in dotted labels is refused, not repaired.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Longest host name, as DNS has it.
pub const MAX_HOST: usize = 253;
/// Longest label of one.
const MAX_LABEL: usize = 63;
/// Most hosts [`typed_hosts`] gives, however long the line.
pub const MAX_TYPED: usize = 8;

/// Why a text is not a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostError {
    Empty,
    TooLong,
    /// A character outside ASCII: no international names, in letters or lookalikes.
    NotAscii,
    /// A user name before an `@`: `www.youtube.com@evil.example` is evil's host.
    UserInfo,
    /// A path, query or fragment: a host is only the name.
    Path,
    /// A port: the destination's port is not part of the host.
    Port,
    /// A `[` for an IPv6 literal, which is not taken.
    Ipv6,
    /// A label that is empty, too long, or has a character other than a letter, a
    /// digit or a hyphen, or starts or ends with a hyphen; or a number that is not
    /// an address.
    BadLabel,
}

impl HostError {
    /// In words for the person.
    pub fn words(self) -> &'static str {
        match self {
            Self::Empty => "it is empty",
            Self::TooLong => "it is too long",
            Self::NotAscii => "it has characters that are not plain letters or digits",
            Self::UserInfo => "it has a user name before an @",
            Self::Path => "it has a path or a query after the name",
            Self::Port => "it has a port",
            Self::Ipv6 => "it is an IPv6 address",
            Self::BadLabel => "it is not a plain dotted host name",
        }
    }
}

/// A host name or an IPv4 address, lowercase, with no trailing dot.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Host(String);

impl Host {
    /// `text` as a host, or why it is not one. One trailing dot (a fully qualified
    /// name) is dropped.
    pub fn parse(text: &str) -> Result<Self, HostError> {
        if text.is_empty() {
            return Err(HostError::Empty);
        }
        if !text.is_ascii() {
            return Err(HostError::NotAscii);
        }
        if text.len() > MAX_HOST + 1 {
            return Err(HostError::TooLong);
        }
        if text.contains('@') {
            return Err(HostError::UserInfo);
        }
        if text.contains(['/', '?', '#', '\\']) {
            return Err(HostError::Path);
        }
        if text.starts_with('[') {
            return Err(HostError::Ipv6);
        }
        if text.contains(':') {
            return Err(HostError::Port);
        }
        let name = text.strip_suffix('.').unwrap_or(text);
        if name.is_empty() {
            return Err(HostError::Empty);
        }
        if name.len() > MAX_HOST {
            return Err(HostError::TooLong);
        }
        let name = name.to_ascii_lowercase();
        if !name.split('.').all(label_ok) {
            return Err(HostError::BadLabel);
        }
        // A name that is all numbers is an address, and must be a whole one.
        let numeric = name
            .split('.')
            .all(|l| l.bytes().all(|b| b.is_ascii_digit()));
        if numeric && !is_ipv4(&name) {
            return Err(HostError::BadLabel);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// An IPv4 address, written as four numbers.
    pub fn is_ip(&self) -> bool {
        is_ipv4(&self.0)
    }

    /// Whether a label is an encoded international name (`xn--`), which can look
    /// like another site's: the person is told.
    pub fn is_punycode(&self) -> bool {
        self.0.split('.').any(|label| label.starts_with("xn--"))
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn label_ok(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_LABEL
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-'
        })
}

/// Four numbers 0 to 255, with no leading zero (`010` reads differently in
/// different places, so it is not an address here).
fn is_ipv4(name: &str) -> bool {
    let mut count = 0;
    for part in name.split('.') {
        count += 1;
        let digits =
            !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit());
        let plain = part == "0" || !part.starts_with('0');
        if !digits || !plain || part.parse::<u16>().is_ok_and(|n| n > 255) {
            return false;
        }
    }
    count == 4
}

/// Characters that end a word of the line before a host starts or after it ends:
/// spaces and the brackets and quotes a host is written inside.
fn separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            ',' | ';' | '(' | ')' | '<' | '>' | '"' | '\'' | '`' | '[' | ']' | '{' | '}'
        )
}

/// The host a word of the line names, if it names one: bare, with a port, or as
/// the host of an `http://` or `https://` address. A host has a dot and a last
/// label of letters (or an address): `e.g` and `3.5` are not names. An `a@b.c` is
/// not a host. A bare word that ends like a file (`readme.md`, `build.sh`, `x.zip`,
/// several of which are also countries' domains) is a file unless it is written as
/// an address: it grounds nothing, and a fetch from it asks.
fn named_by(word: &str) -> Option<Host> {
    let lower = word.to_ascii_lowercase();
    let scheme = lower.starts_with("https://") || lower.starts_with("http://");
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or("");
    if rest.contains('@') {
        return None;
    }
    // Trailing sentence marks, then a port.
    let rest = rest.trim_end_matches(['.', ':', '!', '?', ';', ',']);
    let rest = match rest.rsplit_once(':') {
        Some((name, port))
            if !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit()) =>
        {
            name
        }
        _ => rest,
    };
    if !rest.contains('.') || rest.starts_with('.') {
        return None;
    }
    let host = Host::parse(rest).ok()?;
    let last = host.as_str().rsplit('.').next().unwrap_or("");
    let name_like = last.len() >= 2
        && last
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-' || b.is_ascii_digit());
    let tld_letters = last.starts_with("xn--") || last.bytes().all(|b| b.is_ascii_lowercase());
    let is_file = !scheme && FILE_ENDINGS.contains(&last);
    (host.is_ip() || (name_like && tld_letters && !is_file)).then_some(host)
}

/// Endings of file names, which a bare word is taken for rather than a host.
const FILE_ENDINGS: &[&str] = &[
    "c", "cc", "cs", "csv", "dll", "doc", "exe", "gif", "gz", "h", "img", "iso", "jpg", "js",
    "json", "log", "md", "mov", "pdf", "pl", "pm", "png", "py", "rb", "rs", "sh", "so", "tar",
    "toml", "ts", "txt", "yaml", "yml", "zip",
];

/// The hosts the user's line itself names, in order, each once, at most
/// [`MAX_TYPED`]: what the user typed is grounded.
pub fn typed_hosts(line: &str) -> Vec<Host> {
    let mut out: Vec<Host> = Vec::new();
    for word in line.split(separator).filter(|w| !w.is_empty()) {
        if let Some(host) = named_by(word)
            && !out.contains(&host)
        {
            out.push(host);
            if out.len() == MAX_TYPED {
                break;
            }
        }
    }
    out
}

/// One word of the user's line, and whether it counts. The agent core's consent
/// reads the line in clauses and knows which words sit in one that negates
/// ("don't use weather", "never fetch youtube.com", "without weather"): those
/// words do not count, and a host they name is not grounded by them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineWord {
    /// Lowercase, as consent writes its words: letters, digits and apostrophes.
    pub text: String,
    /// False for a word in a negated clause.
    pub counts: bool,
}

impl LineWord {
    pub fn new(text: &str, counts: bool) -> Self {
        Self {
            text: text.to_lowercase(),
            counts,
        }
    }

    /// Every word of `line`, all counting: for a caller that applies no clause
    /// rules.
    pub fn all(line: &str) -> Vec<Self> {
        split_words(line)
            .into_iter()
            .map(|text| Self { text, counts: true })
            .collect()
    }
}

/// The words of a line as consent reads them: letters, digits and apostrophes,
/// lowercase, everything else a separator ("what's the weather in Leeds?" is
/// `what's`, `the`, `weather`, `in`, `leeds`). At most 256.
fn split_words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|word| word.trim_matches('\''))
        .filter(|word| !word.is_empty())
        .take(256)
        .map(str::to_lowercase)
        .collect()
}

/// The words a host is written with: its labels, split at hyphens too.
pub(crate) fn host_words(host: &Host) -> Vec<String> {
    split_words(host.as_str())
}
