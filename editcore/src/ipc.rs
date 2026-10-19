//! What the kernel sends the edit service, one IPC message each, and how the
//! kernel reads the service's answer to a sync.

use crate::wire::{Reader, SyncRequest, label, name, read_sync, sync_fields};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::valid_commit;

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
