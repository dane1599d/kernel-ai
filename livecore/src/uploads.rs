//! Uploads (M9b): versions the edit loop built and put into the store at run time.
//!
//! The store names each one: `e1`, `e2`, ... per component per boot, advanced only
//! by a successful upload and never reused. An upload must say in its manifest
//! that it is that label of this component, keep the active version's interface,
//! come from a stored base, and name the git commit holding its source. The last
//! [`KEEP_UPLOADS`] are kept, within [`MAX_UPLOAD_TOTAL`] for all components.
//!
//! The outbox records each commit and rollback the ledger accepts, so the edit
//! service can move the git branch that names what runs; a job stays until the host
//! acknowledges it with the commit it moved to.

use crate::store::{Component, Origin, Version};
use crate::{Reason, artifact};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{
    KEEP_UPLOADS, MAX_UPLOAD, MAX_UPLOAD_TOTAL, UPLOAD_FILES_MAX, UPLOAD_LINES_MAX, UPLOAD_PREFIX,
    UPLOAD_SOURCE_MAX, valid_commit, valid_label,
};

/// Sync jobs kept unacknowledged; the oldest is dropped when one more comes.
pub const OUTBOX: usize = 16;
/// Acknowledged jobs kept, for `:comp` and the tests.
const SYNCED: usize = 16;

/// What the store needs from an upload's manifest, read before taking its lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub component: String,
    pub version: String,
    pub interface: String,
    pub source: String,
    /// The source's file paths, sorted.
    pub files: Vec<String>,
}

/// The sorted file paths of an artifact's source, when it is within the
/// editor's limits: [`UPLOAD_SOURCE_MAX`] bytes, [`UPLOAD_FILES_MAX`] files and
/// [`UPLOAD_LINES_MAX`] lines a file.
pub fn source_paths(source: &[u8]) -> Option<Vec<String>> {
    let files = artifact::source_files(source).ok()?;
    let lines = |bytes: &[u8]| bytes.split(|&b| b == b'\n').count();
    let fits = source.len() <= UPLOAD_SOURCE_MAX
        && files.len() <= UPLOAD_FILES_MAX
        && files
            .iter()
            .all(|(_, bytes)| lines(bytes) <= UPLOAD_LINES_MAX + 1);
    if !fits {
        return None;
    }
    let mut paths: Vec<String> = files.iter().map(|(path, _)| path.to_string()).collect();
    paths.sort();
    Some(paths)
}

/// Reads an upload's manifest and file list: TooBig over [`MAX_UPLOAD`], Invalid
/// when the artifact does not parse or its source is over the editor's limits.
pub fn meta(bytes: &[u8]) -> Result<Meta, Error> {
    if bytes.len() > MAX_UPLOAD {
        return Err(Error::TooBig);
    }
    let parsed = artifact::parse(bytes).map_err(|_| Error::Invalid)?;
    let files = source_paths(parsed.source).ok_or(Error::Invalid)?;
    let manifest = parsed.manifest;
    Ok(Meta {
        component: manifest.component,
        version: manifest.version,
        interface: manifest.interface,
        source: manifest.source,
        files,
    })
}

/// The upload rules, against the label the store assigns next.
pub(crate) fn check(
    meta: &Meta,
    name: &str,
    next: &str,
    active: Option<&Version>,
    base: &str,
    commit: &str,
    stored: impl Fn(&str) -> bool,
) -> Result<(), Error> {
    let active = active.ok_or(Error::Invalid)?;
    let ok = meta.component == name
        && meta.version == next
        && !active.interface.is_empty()
        && meta.interface == active.interface
        && meta.files == active.files
        && valid_label(base)
        && stored(base)
        && valid_commit(commit);
    if ok { Ok(()) } else { Err(Error::Invalid) }
}

/// The uploads to evict to make room for one of `size` bytes: failed ones first,
/// then the oldest, never the active version or the candidate. Busy when the count
/// cannot be met, NoMemory when the bytes cannot.
pub(crate) fn victims(
    component: &Component,
    size: usize,
    used_elsewhere: usize,
) -> Result<Vec<String>, Error> {
    let ledger = &component.ledger;
    let keep = |label: &str| ledger.active() == Some(label) || ledger.candidate() == Some(label);
    let mut uploads: Vec<(bool, u64, &Version)> = component
        .versions()
        .iter()
        .filter_map(|v| match v.origin {
            Origin::Upload { seq, .. } => Some((ledger.is_bad(&v.label).is_none(), seq, v)),
            Origin::Boot => None,
        })
        .collect();
    let mut held: usize = uploads.iter().map(|(.., v)| v.bytes.as_slice().len()).sum();
    let mut count = uploads.len();
    // Failed first (false sorts before true), then the oldest.
    uploads.sort_by_key(|&(good, seq, _)| (good, seq));
    let mut chosen = Vec::new();
    for (_, _, version) in uploads {
        let fits = count < KEEP_UPLOADS && used_elsewhere + held + size <= MAX_UPLOAD_TOTAL;
        if fits {
            break;
        }
        if keep(&version.label) {
            continue;
        }
        count -= 1;
        held -= version.bytes.as_slice().len();
        chosen.push(version.label.clone());
    }
    if count >= KEEP_UPLOADS {
        return Err(Error::Busy);
    }
    if used_elsewhere + held + size > MAX_UPLOAD_TOTAL {
        return Err(Error::NoMemory);
    }
    Ok(chosen)
}

/// A component's upload numbering, counts and outbox.
#[derive(Clone, Debug)]
pub struct Uploads {
    /// The number the next upload's label carries.
    next: u64,
    pub accepted: u64,
    /// Uploads the rules refused.
    pub rejected: u64,
    pub evicted: u64,
    pub outbox: Outbox,
}

impl Default for Uploads {
    fn default() -> Self {
        Self::new()
    }
}

impl Uploads {
    pub const fn new() -> Self {
        Self {
            next: 1,
            accepted: 0,
            rejected: 0,
            evicted: 0,
            outbox: Outbox::new(),
        }
    }

    pub fn next_label(&self) -> String {
        format!("{UPLOAD_PREFIX}{}", self.next)
    }

    pub(crate) fn take_seq(&mut self) -> u64 {
        let seq = self.next;
        self.next += 1;
        seq
    }
}

/// How a swap ended, for git.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Commit,
    Rollback(Reason),
}

/// One commit or rollback for the edit service to record in git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncJob {
    pub seq: u64,
    pub label: String,
    pub outcome: Outcome,
    /// The ledger's generation after the event.
    pub generation: u64,
    /// The version's source hash, from its manifest; empty when unknown.
    pub source: String,
    /// The upload's commit; empty for a boot version.
    pub commit: String,
    /// The commit git recorded, once acknowledged.
    pub sha: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Outbox {
    enabled: bool,
    jobs: VecDeque<SyncJob>,
    synced: VecDeque<SyncJob>,
    next_seq: u64,
    /// Jobs dropped unacknowledged because the outbox was full.
    pub dropped: u64,
}

impl Default for Outbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Outbox {
    pub const fn new() -> Self {
        Self {
            enabled: false,
            jobs: VecDeque::new(),
            synced: VecDeque::new(),
            next_seq: 1,
            dropped: 0,
        }
    }

    /// Jobs are recorded only while sync is on: when the kernel has a build service.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn push(
        &mut self,
        label: &str,
        outcome: Outcome,
        generation: u64,
        source: String,
        commit: String,
    ) {
        if !self.enabled {
            return;
        }
        if self.jobs.len() == OUTBOX {
            self.jobs.pop_front();
            self.dropped += 1;
        }
        self.jobs.push_back(SyncJob {
            seq: self.next_seq,
            label: label.to_string(),
            outcome,
            generation,
            source,
            commit,
            sha: None,
        });
        self.next_seq += 1;
    }

    /// The oldest job not yet acknowledged.
    pub fn peek(&self) -> Option<&SyncJob> {
        self.jobs.front()
    }

    pub fn pending(&self) -> usize {
        self.jobs.len()
    }

    /// Acknowledges job `seq` with the commit git recorded; false if it is not the
    /// oldest pending job.
    pub fn ack(&mut self, seq: u64, sha: &str) -> bool {
        if self.jobs.front().is_none_or(|job| job.seq != seq) {
            return false;
        }
        let Some(mut job) = self.jobs.pop_front() else {
            return false;
        };
        job.sha = Some(sha.to_string());
        if self.synced.len() == SYNCED {
            self.synced.pop_front();
        }
        self.synced.push_back(job);
        true
    }

    /// Gives up on job `seq`, the oldest pending one, which git keeps refusing;
    /// counted as dropped. False if it is not the oldest.
    pub fn drop_head(&mut self, seq: u64) -> bool {
        if self.jobs.front().is_none_or(|job| job.seq != seq) {
            return false;
        }
        self.jobs.pop_front();
        self.dropped += 1;
        true
    }

    /// Acknowledged jobs, oldest first.
    pub fn synced(&self) -> impl Iterator<Item = &SyncJob> {
        self.synced.iter()
    }
}

/// The upload `:apply` would swap to: the newest one, unless it is active, on
/// probation, or failed before. The refusal is a line for the terminal.
pub fn apply_candidate(component: &Component) -> Result<String, String> {
    let name = &component.name;
    let ledger = &component.ledger;
    let newest = component
        .versions()
        .iter()
        .filter_map(|v| match v.origin {
            Origin::Upload { seq, .. } => Some((seq, v.label.as_str())),
            Origin::Boot => None,
        })
        .max_by_key(|&(seq, _)| seq)
        .map(|(_, label)| label)
        .ok_or_else(|| format!("{name} has no uploads; build one in :edit {name}"))?;
    if ledger.disabled {
        return Err(format!("{name} is disabled; :call {name} ctl reset first"));
    }
    if let Some(candidate) = ledger.candidate() {
        return Err(format!("busy: {name} {candidate} is on probation"));
    }
    if ledger.active() == Some(newest) {
        return Err(format!("nothing new: {newest} is active"));
    }
    if let Some(why) = ledger.is_bad(newest) {
        return Err(format!(
            "{newest} rolled back before ({}); :swap {name} {newest} force retries it",
            why.name()
        ));
    }
    if ledger.active().is_none() {
        return Err(format!("{name} has not started yet"));
    }
    Ok(newest.to_string())
}

/// What `:apply` shows before it asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// The upload it would swap to.
    pub label: String,
    /// The version the change is computed against.
    pub active: String,
    /// A header, then the change, for a screen `cols` wide.
    pub lines: Vec<String>,
}

/// The newest upload and its change against the active version, both from the
/// store. Refused when it cannot be shown in full within `rows` by `cols`: a
/// confirmation shows all of what it asks about.
pub fn proposal(component: &Component, cols: usize, rows: usize) -> Result<Proposal, String> {
    let name = &component.name;
    let label = apply_candidate(component)?;
    let active = component.ledger.active().unwrap_or("").to_string();
    let (Some(upload), Some(running)) = (component.version(&label), component.version(&active))
    else {
        return Err(format!("{name} {label} is no longer stored"));
    };
    let Origin::Upload { base, commit, .. } = &upload.origin else {
        return Err(format!("{label} is not an upload"));
    };
    let source = |bytes: &[u8]| artifact::parse(bytes).map(|a| a.source.to_vec());
    let new = source(upload.bytes.as_slice()).map_err(|why| format!("{label}: {why}"))?;
    let old = source(running.bytes.as_slice()).map_err(|why| format!("{active}: {why}"))?;
    let mut lines = alloc::vec![format!(
        "apply {name} {label} (built from {base}, commit {}) over active {active}",
        commit.get(..12).unwrap_or("")
    )];
    // The header takes a row, and the prompt another.
    match crate::diff::view(&old, &new, cols, rows.saturating_sub(2)) {
        Ok(view) if view.is_empty() => lines.push(String::from("no source changes")),
        Ok(view) => lines.extend(view),
        Err(crate::diff::ViewError::TooLarge | crate::diff::ViewError::TooWide) => {
            return Err(format!(
                "{label}'s change does not fit one screen; review it with diff in :edit {name}, then :swap {name} {label}"
            ));
        }
        Err(crate::diff::ViewError::Unshowable) => {
            return Err(format!(
                "{label}'s change holds characters the confirmation cannot show exactly; review it with diff in :edit {name}, then :swap {name} {label}"
            ));
        }
        Err(crate::diff::ViewError::Bundle(why)) => return Err(format!("{label}: {why}")),
    }
    Ok(Proposal {
        label,
        active,
        lines,
    })
}

#[cfg(test)]
mod tests;
