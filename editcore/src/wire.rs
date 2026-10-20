//! What passes between the edit service and the host's build service (the lines
//! the kernel sends the edit service are in [`crate::ipc`]). Each format is strict:
//! a header, then every key in order once, and byte fields framed by their length.
//!
//! ```text
//! kai-build 2            kai-built 1               kai-sync 1           kai-synced 1
//! component notes        result ok|failed|refused  component notes      result moved <ref> <sha>
//! label e1               base 2            (ok)    label e1             result recorded <ref> <sha>
//! base 2                 commit <40 hex>   (ok)    outcome commit       result untracked
//! origin founder|model   log <len>                 outcome rollback trap
//! bundle <len>           <bytes>                   generation 1
//! <bytes>                artifact <len>    (ok)    source <64 hex>|-
//!                        <bytes>                   commit <40 hex>|-
//! ```
//!
//! `origin` says who wrote the source: the founder, or the model (a draft was
//! applied in the session). The host builds model source only in its sandbox.

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
pub(crate) struct Reader<'a> {
    pub(crate) rest: &'a [u8],
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

    pub(crate) fn end(&self) -> Result<(), &'static str> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err("bytes after the last field")
        }
    }
}

pub(crate) fn label(text: &str) -> Result<&str, &'static str> {
    if valid_label(text) {
        Ok(text)
    } else {
        Err("a label is malformed")
    }
}

pub(crate) fn name(text: &str) -> Result<&str, &'static str> {
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

/// Who wrote the source a build request carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Founder,
    Model,
}

impl Origin {
    fn name(self) -> &'static str {
        match self {
            Self::Founder => "founder",
            Self::Model => "model",
        }
    }

    fn parse(text: &str) -> Result<Self, &'static str> {
        match text {
            "founder" => Ok(Self::Founder),
            "model" => Ok(Self::Model),
            _ => Err("an origin is neither founder nor model"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRequest<'a> {
    pub component: &'a str,
    /// The label the store will give it, which its manifest must carry.
    pub label: &'a str,
    /// The version its source came from.
    pub base: &'a str,
    pub origin: Origin,
    pub bundle: &'a [u8],
}

pub fn encode_build(request: &BuildRequest<'_>) -> Vec<u8> {
    let mut out = format!(
        "kai-build 2\ncomponent {}\nlabel {}\nbase {}\norigin {}\nbundle {}\n",
        request.component,
        request.label,
        request.base,
        request.origin.name(),
        request.bundle.len()
    )
    .into_bytes();
    out.extend_from_slice(request.bundle);
    out
}

/// A request with no origin, or one this side does not know, does not decode:
/// the host then refuses it rather than guess who wrote the source.
pub fn decode_build(bytes: &[u8]) -> Result<BuildRequest<'_>, &'static str> {
    let mut reader = Reader::new(bytes, "kai-build 2")?;
    let request = BuildRequest {
        component: name(reader.value("component")?)?,
        label: label(reader.value("label")?)?,
        base: label(reader.value("base")?)?,
        origin: Origin::parse(reader.value("origin")?)?,
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
    /// The version the source was built from; ok only.
    pub base: &'a str,
    /// The commit holding the source; ok only.
    pub commit: &'a str,
    pub log: &'a [u8],
    /// Ok only.
    pub artifact: &'a [u8],
}

pub fn encode_built(built: &Built<'_>) -> Vec<u8> {
    let mut out = format!("kai-built 1\nresult {}\n", built.result.name()).into_bytes();
    if built.result == BuildResult::Ok {
        let fields = format!("base {}\ncommit {}\n", built.base, built.commit);
        out.extend_from_slice(fields.as_bytes());
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
    let (base, commit) = if ok {
        let base = label(reader.value("base")?)?;
        let commit = Some(reader.value("commit")?)
            .filter(|c| valid_commit(c))
            .ok_or("the commit is malformed")?;
        (base, commit)
    } else {
        ("", "")
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
        base,
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

pub(crate) fn sync_fields(request: &SyncRequest<'_>) -> String {
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

pub(crate) fn read_sync<'a>(reader: &mut Reader<'a>) -> Result<SyncRequest<'a>, &'static str> {
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

/// Whether `request` is a build, as `http::post` writes one: only BUILD_START
/// sends these.
pub fn is_build_request(request: &[u8]) -> bool {
    request.starts_with(b"POST /build HTTP/1.1\r\n")
}

/// Whether `request` is a git sync, as `http::post` writes one.
pub fn is_sync_request(request: &[u8]) -> bool {
    request.starts_with(b"POST /sync HTTP/1.1\r\n")
}

/// The body of a whole `200` response from the build service, without allocating:
/// a `Content-Length` that covers exactly the bytes after the headers, and no
/// other transfer encoding. The build service always answers this way.
pub fn http_body(response: &[u8]) -> Result<&[u8], &'static str> {
    if !response.starts_with(b"HTTP/1.1 200 ") {
        return Err("the build reply is not a 200");
    }
    let end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the build reply has no end of headers")?;
    let head = core::str::from_utf8(&response[..end]).map_err(|_| "the headers are not text")?;
    let body = &response[end + 4..];
    let mut length = None;
    for line in head.split("\r\n").skip(1) {
        let (name, value) = line.split_once(':').ok_or("a header has no colon")?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("the build reply has a transfer encoding");
        }
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err("the build reply has two lengths");
            }
            length = Some(value.trim().parse::<usize>().map_err(|_| "a bad length")?);
        }
    }
    match length {
        Some(length) if length == body.len() => Ok(body),
        _ => Err("the build reply's length does not match its body"),
    }
}

/// Whether `artifact`, `base` and `commit` are exactly what the build service
/// answered in `response`, the raw reply to a `POST /build`. The kernel checks
/// every upload this way, so what it shows and runs is what the host built.
pub fn check_built_response(
    response: &[u8],
    artifact: &[u8],
    base: &str,
    commit: &str,
) -> Result<(), &'static str> {
    let built = decode_built(http_body(response)?)?;
    if built.result != BuildResult::Ok {
        return Err("the last build did not succeed");
    }
    if built.artifact != artifact {
        return Err("the artifact is not the one the build service returned");
    }
    if built.base != base {
        return Err("the base is not the build's");
    }
    if built.commit != commit {
        return Err("the commit is not the build's");
    }
    Ok(())
}
