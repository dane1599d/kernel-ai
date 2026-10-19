//! The calls the editor uses (M9b): upload a version, read the store, and relay a
//! control request; and the label and commit rules they check.

use super::{COMMIT_LEN, MAX_CONTROL, MAX_LABEL, UPLOAD_PREFIX, label_text, phase};

/// Argument of [`crate::call::COMP_PUT`].
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Put {
    /// The component.
    pub name_ptr: u64,
    pub name_len: u64,
    /// The artifact, at most [`MAX_UPLOAD`](super::MAX_UPLOAD); its manifest names the label.
    pub artifact_ptr: u64,
    pub artifact_len: u64,
    /// The stored version it was built from.
    pub base_ptr: u64,
    pub base_len: u64,
    /// The git commit holding its source, [`COMMIT_LEN`] lowercase hex.
    pub commit_ptr: u64,
    pub commit_len: u64,
}

/// What [`crate::call::COMP_READ`] reads, as [`Read::what`].
pub mod read {
    /// The component's [`super::Info`].
    pub const INFO: u64 = 1;
    /// A version's bytes; an empty label means the active version.
    pub const ARTIFACT: u64 = 2;
}

/// Argument of [`crate::call::COMP_READ`].
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Read {
    /// A [`read`] constant.
    pub what: u64,
    pub name_ptr: u64,
    pub name_len: u64,
    pub label_ptr: u64,
    pub label_len: u64,
    /// For INFO, room for an [`Info`]; for ARTIFACT, the bytes when they fit.
    pub buf_ptr: u64,
    pub buf_cap: u64,
}

/// A component as [`read::INFO`] reports it.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Info {
    /// A [`phase`].
    pub phase: u64,
    /// Commits so far.
    pub generation: u64,
    /// [`Info::DISABLED`].
    pub flags: u64,
    /// Uploads stored now.
    pub uploads: u64,
    pub active_len: u64,
    pub candidate_len: u64,
    pub next_len: u64,
    pub active: [u8; MAX_LABEL],
    pub candidate: [u8; MAX_LABEL],
    /// The label the next upload must carry.
    pub next: [u8; MAX_LABEL],
}

impl Info {
    pub const DISABLED: u64 = 1;

    pub const fn empty() -> Self {
        Self {
            phase: phase::EMPTY,
            generation: 0,
            flags: 0,
            uploads: 0,
            active_len: 0,
            candidate_len: 0,
            next_len: 0,
            active: [0; MAX_LABEL],
            candidate: [0; MAX_LABEL],
            next: [0; MAX_LABEL],
        }
    }

    pub fn active(&self) -> &str {
        label_text(&self.active, self.active_len)
    }

    pub fn candidate(&self) -> &str {
        label_text(&self.candidate, self.candidate_len)
    }

    pub fn next(&self) -> &str {
        label_text(&self.next, self.next_len)
    }
}

/// Copies `label` into a fixed field; its length.
pub fn put_label(into: &mut [u8; MAX_LABEL], label: &str) -> u64 {
    let bytes = label.as_bytes();
    let len = bytes.len().min(MAX_LABEL);
    into[..len].copy_from_slice(&bytes[..len]);
    len as u64
}

/// Whether `label` is a version label: 1 to 16 of `a-z`, `0-9` and `-`.
pub fn valid_label(label: &str) -> bool {
    (1..=MAX_LABEL).contains(&label.len())
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The number of upload label `e<n>`, or None for any other label.
pub fn upload_seq(label: &str) -> Option<u64> {
    let digits = label.strip_prefix(UPLOAD_PREFIX)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Whether `label` has the form of an upload label, `e` and digits. Boot modules
/// may not use it.
pub fn is_upload_label(label: &str) -> bool {
    upload_seq(label).is_some()
}

/// Whether `text` is a git commit id: [`COMMIT_LEN`] lowercase hex digits.
pub fn valid_commit(text: &str) -> bool {
    text.len() == COMMIT_LEN
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Argument of [`crate::call::COMP_CONTROL`].
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Control {
    /// The component.
    pub name_ptr: u64,
    pub name_len: u64,
    /// What follows `ctl `, at most [`MAX_CONTROL`]; see [`relayable`].
    pub request_ptr: u64,
    pub request_len: u64,
    /// Room for the reply; as much as fits is copied, and its length returned.
    pub reply_ptr: u64,
    pub reply_cap: u64,
}

/// Whether the kernel relays `ctl <request>` to a component for a PUBLISH holder:
/// only `status`, `source [file]` and `try <version>`, which change nothing.
/// `swap`, `rollback`, `reset` and anything else never pass, whatever the
/// component's own parser would accept.
pub fn relayable(request: &str) -> bool {
    if request.len() > MAX_CONTROL || !request.bytes().all(|b| (b' '..=b'~').contains(&b)) {
        return false;
    }
    let mut words = request.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("status"), None, None) | (Some("source"), _, None) => true,
        (Some("try"), Some(label), None) => valid_label(label),
        _ => false,
    }
}
