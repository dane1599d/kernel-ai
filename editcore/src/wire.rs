//! What passes between the edit service and the host's build service, and the
//! lines the kernel sends the edit service. Each format is strict: a header, then
//! every key in order once, and byte fields framed by their length.
//!
//! ```text
//! kai-build 1            kai-built 1               kai-sync 1           kai-synced 1
//! component notes        result ok|failed|refused  component notes      result moved <ref> <sha>
//! label e1               commit <40 hex>   (ok)    label e1             result recorded <ref> <sha>
//! base 2                 log <len>                 outcome commit       result untracked
//! bundle <len>           <bytes>                   outcome rollback trap
//! <bytes>                artifact <len>    (ok)    generation 1
//!                        <bytes>                   source <64 hex>|-
//!                                                  commit <40 hex>|-
//! ```

use crate::{MAX_BUNDLE, MAX_LOG};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::{MAX_UPLOAD, Reason, valid_commit, valid_label};
use livecore::uploads::Outcome;

/// A SHA-256 as 64 lowercase hex digits.
pub fn valid_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Reads `key value\n` lines and length-framed byte fields, in order.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], header: &str) -> Result<Self, &'static str> {
        let rest = bytes
            .strip_prefix(header.as_bytes())
            .and_then(|rest| rest.strip_prefix(b"\n"))
            .ok_or("wrong header")?;
        Ok(Self { rest })
    }

    fn line(&mut self) -> Result<&'a str, &'static str> {
        let end = self
            .rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or("a line has no end")?;
        let line = core::str::from_utf8(&self.rest[..end]).map_err(|_| "a line is not text")?;
        self.rest = &self.rest[end + 1..];
        Ok(line)
    }

    /// The value of the next line, which must be `key value`.
    fn value(&mut self, key: &str) -> Result<&'a str, &'static str> {
        self.line()?
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(' '))
            .filter(|value| !value.is_empty())
            .ok_or("a key is missing or out of order")
    }

    fn bytes(&mut self, key: &str, max: usize) -> Result<&'a [u8], &'static str> {
        let len: usize = self
            .value(key)?
            .parse()
            .map_err(|_| "a length is not a number")?;
        if len > max {
            return Err("a field is too large");
        }
        if len > self.rest.len() {
            return Err("a field runs past the end");
        }
        let (field, rest) = self.rest.split_at(len);
        self.rest = rest;
        Ok(field)
    }

    fn end(&self) -> Result<(), &'static str> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err("bytes after the last field")
        }
    }
}

fn label(text: &str) -> Result<&str, &'static str> {
    if valid_label(text) {
        Ok(text)
    } else {
        Err("a label is malformed")
    }
}

fn name(text: &str) -> Result<&str, &'static str> {
    let ok = !text.is_empty()
        && text.len() <= kabi::MAX_NAME
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(text)
    } else {
        Err("a name is malformed")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRequest<'a> {
    pub component: &'a str,
    /// The label the store will give it, which its manifest must carry.
    pub label: &'a str,
    /// The version its source came from.
    pub base: &'a str,
    pub bundle: &'a [u8],
}

pub fn encode_build(request: &BuildRequest<'_>) -> Vec<u8> {
    let mut out = format!(
        "kai-build 1\ncomponent {}\nlabel {}\nbase {}\nbundle {}\n",
        request.component,
        request.label,
        request.base,
        request.bundle.len()
    )
    .into_bytes();
    out.extend_from_slice(request.bundle);
    out
}

pub fn decode_build(bytes: &[u8]) -> Result<BuildRequest<'_>, &'static str> {
    let mut reader = Reader::new(bytes, "kai-build 1")?;
    let request = BuildRequest {
        component: name(reader.value("component")?)?,
        label: label(reader.value("label")?)?,
        base: label(reader.value("base")?)?,
        bundle: reader.bytes("bundle", MAX_BUNDLE)?,
    };
    reader.end()?;
    Ok(request)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildResult {
    Ok,
    /// The compiler refused it; the log says why.
    Failed,
    /// The service refused it before compiling (an edit rule, an unknown base).
    Refused,
}

impl BuildResult {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Refused => "refused",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built<'a> {
    pub result: BuildResult,
    /// The commit holding the source; ok only.
    pub commit: &'a str,
    pub log: &'a [u8],
    /// Ok only.
    pub artifact: &'a [u8],
}

pub fn encode_built(built: &Built<'_>) -> Vec<u8> {
    let mut out = format!("kai-built 1\nresult {}\n", built.result.name()).into_bytes();
    if built.result == BuildResult::Ok {
        out.extend_from_slice(format!("commit {}\n", built.commit).as_bytes());
    }
    out.extend_from_slice(format!("log {}\n", built.log.len()).as_bytes());
    out.extend_from_slice(built.log);
    if built.result == BuildResult::Ok {
        out.extend_from_slice(format!("artifact {}\n", built.artifact.len()).as_bytes());
        out.extend_from_slice(built.artifact);
    }
    out
}

pub fn decode_built(bytes: &[u8]) -> Result<Built<'_>, &'static str> {
    let mut reader = Reader::new(bytes, "kai-built 1")?;
    let result = match reader.value("result")? {
        "ok" => BuildResult::Ok,
        "failed" => BuildResult::Failed,
        "refused" => BuildResult::Refused,
        _ => return Err("an unknown result"),
    };
    let ok = result == BuildResult::Ok;
    let commit = if ok {
        Some(reader.value("commit")?)
            .filter(|c| valid_commit(c))
            .ok_or("the commit is malformed")?
    } else {
        ""
    };
    let log = reader.bytes("log", MAX_LOG)?;
    let artifact = if ok {
        reader.bytes("artifact", MAX_UPLOAD)?
    } else {
        &[]
    };
    reader.end()?;
    Ok(Built {
        result,
        commit,
        log,
        artifact,
    })
}

/// A commit or rollback for git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncRequest<'a> {
    pub component: &'a str,
    pub label: &'a str,
    pub outcome: Outcome,
    pub generation: u64,
    /// The version's source hash; empty when unknown.
    pub source: &'a str,
    /// The upload's commit; empty for a boot version.
    pub commit: &'a str,
}

fn or_dash(text: &str) -> &str {
    if text.is_empty() { "-" } else { text }
}

fn outcome_text(outcome: Outcome) -> String {
    match outcome {
        Outcome::Commit => String::from("commit"),
        Outcome::Rollback(reason) => format!("rollback {}", reason.name()),
    }
}

fn parse_outcome(text: &str) -> Result<Outcome, &'static str> {
    if text == "commit" {
        return Ok(Outcome::Commit);
    }
    let reason = text.strip_prefix("rollback ").ok_or("an unknown outcome")?;
    Reason::ALL
        .into_iter()
        .find(|r| r.name() == reason)
        .map(Outcome::Rollback)
        .ok_or("an unknown reason")
}

fn sync_fields(request: &SyncRequest<'_>) -> String {
    format!(
        "component {}\nlabel {}\noutcome {}\ngeneration {}\nsource {}\ncommit {}\n",
        request.component,
        request.label,
        outcome_text(request.outcome),
        request.generation,
        or_dash(request.source),
        or_dash(request.commit)
    )
}

pub fn encode_sync(request: &SyncRequest<'_>) -> Vec<u8> {
    format!("kai-sync 1\n{}", sync_fields(request)).into_bytes()
}

fn read_sync<'a>(reader: &mut Reader<'a>) -> Result<SyncRequest<'a>, &'static str> {
    let component = name(reader.value("component")?)?;
    let label = label(reader.value("label")?)?;
    let outcome = parse_outcome(reader.value("outcome")?)?;
    let generation = reader
        .value("generation")?
        .parse()
        .map_err(|_| "the generation is not a number")?;
    let source = match reader.value("source")? {
        "-" => "",
        hash if valid_hash(hash) => hash,
        _ => return Err("the source hash is malformed"),
    };
    let commit = match reader.value("commit")? {
        "-" => "",
        commit if valid_commit(commit) => commit,
        _ => return Err("the commit is malformed"),
    };
    Ok(SyncRequest {
        component,
        label,
        outcome,
        generation,
        source,
        commit,
    })
}

pub fn decode_sync(bytes: &[u8]) -> Result<SyncRequest<'_>, &'static str> {
    let mut reader = Reader::new(bytes, "kai-sync 1")?;
    let request = read_sync(&mut reader)?;
    reader.end()?;
    Ok(request)
}

/// What git recorded for a sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Synced<'a> {
    /// A commit moved the component's branch.
    Moved { reference: &'a str, sha: &'a str },
    /// A rollback was recorded under its own ref.
    Recorded { reference: &'a str, sha: &'a str },
    /// No commit is known for that source.
    Untracked,
}

pub fn encode_synced(synced: &Synced<'_>) -> Vec<u8> {
    let result = match synced {
        Synced::Moved { reference, sha } => format!("moved {reference} {sha}"),
        Synced::Recorded { reference, sha } => format!("recorded {reference} {sha}"),
        Synced::Untracked => String::from("untracked"),
    };
    format!("kai-synced 1\nresult {result}\n").into_bytes()
}

fn reference(text: &str) -> Result<&str, &'static str> {
    let ok = text.starts_with("refs/")
        && text.len() <= 128
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
        && !text.contains("..");
    if ok {
        Ok(text)
    } else {
        Err("a ref is malformed")
    }
}

pub fn decode_synced(bytes: &[u8]) -> Result<Synced<'_>, &'static str> {
    let mut reader = Reader::new(bytes, "kai-synced 1")?;
    let result = reader.value("result")?;
    reader.end()?;
    let mut parts = result.split(' ');
    let synced = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("untracked"), None, None, None) => Synced::Untracked,
        (Some(kind @ ("moved" | "recorded")), Some(r), Some(sha), None) => {
            let reference = reference(r)?;
            if !valid_commit(sha) {
                return Err("the commit is malformed");
            }
            match kind {
                "moved" => Synced::Moved { reference, sha },
                _ => Synced::Recorded { reference, sha },
            }
        }
        _ => return Err("an unknown result"),
    };
    Ok(synced)
}

/// What the kernel sends the edit service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ipc<'a> {
    /// Open (or resume) a session on a version; the active one by default.
    Open {
        component: &'a str,
        label: Option<&'a str>,
    },
    /// One line typed in edit mode.
    Cmd(&'a str),
    /// Record outbox job `seq` in git.
    Sync { seq: u64, job: SyncRequest<'a> },
}

pub fn encode_ipc(message: &Ipc<'_>) -> Vec<u8> {
    match message {
        Ipc::Open {
            component,
            label: Some(label),
        } => format!("open {component} {label}").into_bytes(),
        Ipc::Open {
            component,
            label: None,
        } => format!("open {component}").into_bytes(),
        Ipc::Cmd(line) => format!("cmd {line}").into_bytes(),
        Ipc::Sync { seq, job } => format!("sync {seq}\n{}", sync_fields(job)).into_bytes(),
    }
}

pub fn decode_ipc(bytes: &[u8]) -> Result<Ipc<'_>, &'static str> {
    let text = core::str::from_utf8(bytes).map_err(|_| "not text")?;
    if let Some(rest) = text.strip_prefix("cmd ") {
        return Ok(Ipc::Cmd(rest));
    }
    if text == "cmd" {
        return Ok(Ipc::Cmd(""));
    }
    if let Some(rest) = text.strip_prefix("open ") {
        let mut parts = rest.split(' ');
        let component = name(parts.next().unwrap_or(""))?;
        let label = parts.next().map(label).transpose()?;
        if parts.next().is_some() {
            return Err("open takes a component and a label");
        }
        return Ok(Ipc::Open { component, label });
    }
    if let Some(rest) = text.strip_prefix("sync ") {
        let (seq, fields) = rest.split_once('\n').ok_or("a sync has no fields")?;
        let seq = seq.parse().map_err(|_| "the job number is not a number")?;
        let mut reader = Reader {
            rest: fields.as_bytes(),
        };
        let job = read_sync(&mut reader)?;
        reader.end()?;
        return Ok(Ipc::Sync { seq, job });
    }
    Err("an unknown request")
}

/// The edit service's answer to a sync of job `seq`: the commit git recorded, or
/// empty when git tracks no commit for it. Anything else is an error to retry.
pub fn parse_ack(reply: &[u8], seq: u64) -> Result<String, String> {
    let reply =
        core::str::from_utf8(reply).map_err(|_| String::from("a reply that is not text"))?;
    let mut words = reply.split(' ');
    match (
        words.next(),
        words.next(),
        words.next(),
        words.next(),
        words.next(),
    ) {
        (Some("ok"), Some("synced"), Some(n), Some(sha), None)
            if n.parse() == Ok(seq) && valid_commit(sha) =>
        {
            Ok(String::from(sha))
        }
        (Some("ok"), Some("untracked"), Some(n), None, None) if n.parse() == Ok(seq) => {
            Ok(String::new())
        }
        _ => Err(String::from(reply)),
    }
}
