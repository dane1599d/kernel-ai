//! What a caller sends `search` and what it answers, as text: one request, one
//! reply, each a few lines in a fixed order. Both are read as hostile and refused
//! whole when anything is out of shape, and what reads writes back byte for byte.
//!
//! ```text
//! search 1                 the request: its version
//! query <words>            what is looked for: printable, one line
//! since <ns>               optional: only files changed at or after it
//! folder <path>            optional: only files under it, a clean path
//! limit <n>                optional: at most n results, 1 to 20
//!
//! ok <n>                   the reply: n results, best first
//! hit <score> <modified_ns> <offset> <len> <path>    n lines; score in 0..=1000
//! err <why>                or why nothing was looked for
//! ```
//!
//! "last week" is the caller's to resolve, with `semindex::since` from the clock:
//! a request names times only as numbers.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use semindex::scope;
use semindex::search::MAX_HITS;

/// The first line of a request.
pub const MAGIC: &str = "search 1";
/// The longest query, in bytes.
pub const MAX_QUERY: usize = 512;
/// The longest why, in bytes.
pub const MAX_WHY: usize = 200;
/// The best score a hit has.
pub const MAX_SCORE: u32 = 1000;

/// A query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub query: String,
    pub since_ns: Option<u64>,
    pub folder: Option<String>,
    /// At most this many hits; `None` for [`MAX_HITS`].
    pub limit: Option<usize>,
}

impl Request {
    pub fn encode(&self) -> String {
        let mut out = format!("{MAGIC}\nquery {}", self.query);
        if let Some(since) = self.since_ns {
            out.push_str(&format!("\nsince {since}"));
        }
        if let Some(folder) = &self.folder {
            out.push_str(&format!("\nfolder {folder}"));
        }
        if let Some(limit) = self.limit {
            out.push_str(&format!("\nlimit {limit}"));
        }
        out
    }

    /// The request `text` is, if it is exactly what [`Request::encode`] writes for
    /// a request that may be asked.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.split('\n');
        if lines.next() != Some(MAGIC) {
            return None;
        }
        let query = lines.next()?.strip_prefix("query ")?;
        if !words_ok(query, MAX_QUERY) {
            return None;
        }
        let mut request = Self {
            query: String::from(query),
            since_ns: None,
            folder: None,
            limit: None,
        };
        let mut rest = lines.peekable();
        if let Some(since) = rest.next_if(|l| l.starts_with("since ")) {
            request.since_ns = Some(number(&since["since ".len()..])?);
        }
        if let Some(folder) = rest.next_if(|l| l.starts_with("folder ")) {
            let folder = &folder["folder ".len()..];
            if !scope::clean(folder) || !scope::within(folder, scope::HOME) {
                return None;
            }
            request.folder = Some(String::from(folder));
        }
        if let Some(limit) = rest.next_if(|l| l.starts_with("limit ")) {
            let limit: usize = number(&limit["limit ".len()..])?;
            if !(1..=MAX_HITS).contains(&limit) {
                return None;
            }
            request.limit = Some(limit);
        }
        rest.next().is_none().then_some(request)
    }
}

/// One file found, as the reply says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// From 0 to [`MAX_SCORE`], higher is better.
    pub score: u32,
    pub modified_ns: u64,
    /// The best chunk's place in the file, in bytes: what the caller reads as the
    /// snippet, with its own `Dir`.
    pub offset: u64,
    pub len: u32,
    pub path: String,
}

impl Hit {
    /// `semindex`'s hit, its score in thousandths.
    pub fn of(hit: &semindex::Hit) -> Self {
        let score = (hit.score.clamp(0.0, 1.0) * MAX_SCORE as f32 + 0.5) as u32;
        Self {
            score: score.min(MAX_SCORE),
            modified_ns: hit.modified_ns,
            offset: hit.offset,
            len: hit.len,
            path: hit.path.clone(),
        }
    }

    fn line(&self) -> String {
        format!(
            "hit {} {} {} {} {}",
            self.score, self.modified_ns, self.offset, self.len, self.path
        )
    }

    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.strip_prefix("hit ")?.splitn(5, ' ');
        let hit = Self {
            score: number(fields.next()?)?,
            modified_ns: number(fields.next()?)?,
            offset: number(fields.next()?)?,
            len: number(fields.next()?)?,
            path: String::from(fields.next()?),
        };
        let ok = hit.score <= MAX_SCORE
            && scope::clean(&hit.path)
            && hit.path != scope::HOME
            && scope::within(&hit.path, scope::HOME);
        ok.then_some(hit)
    }
}

/// What `search` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The hits, best first: at most [`MAX_HITS`].
    Hits(Vec<Hit>),
    /// Nothing was looked for, and why.
    Err(String),
}

impl Reply {
    /// The reply's text. A why is made one printable line of at most [`MAX_WHY`]
    /// bytes, whatever it held.
    pub fn encode(&self) -> String {
        match self {
            Self::Hits(hits) => hits
                .iter()
                .fold(format!("ok {}", hits.len()), |mut out, hit| {
                    out.push('\n');
                    out.push_str(&hit.line());
                    out
                }),
            Self::Err(why) => format!("err {}", one_line(why)),
        }
    }

    /// The reply `text` is, if it is exactly what [`Reply::encode`] writes.
    pub fn parse(text: &str) -> Option<Self> {
        if let Some(why) = text.strip_prefix("err ") {
            return words_ok(why, MAX_WHY).then(|| Self::Err(String::from(why)));
        }
        let mut lines = text.split('\n');
        let count: usize = number(lines.next()?.strip_prefix("ok ")?)?;
        let hits = lines.map(Hit::parse).collect::<Option<Vec<Hit>>>()?;
        (hits.len() == count && count <= MAX_HITS).then_some(Self::Hits(hits))
    }
}

/// Printable ASCII words of 1 to `max` bytes, not padded.
fn words_ok(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.len() <= max
        && text.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && !text.starts_with(' ')
        && !text.ends_with(' ')
}

/// `text` as one printable line of at most [`MAX_WHY`] bytes: anything else is a
/// `?`, and an empty why says so.
fn one_line(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect();
    out.truncate(MAX_WHY);
    match out.trim() {
        "" => String::from("no reason given"),
        why => String::from(why),
    }
}

/// A number as this module writes one: digits only, no sign, no leading zero.
fn number<T: core::str::FromStr>(text: &str) -> Option<T> {
    let plain = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    plain.then(|| text.parse().ok()).flatten()
}
