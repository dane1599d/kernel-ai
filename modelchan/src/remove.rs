//! `models remove <64 hex>`, the undo of a `model_fetch`: what `updated` may
//! do with a model it brought in. A stored blob leaves the store (`fs`'s
//! `SealedRemove`, which also drops it from every older undo snapshot, so no
//! undo brings it back); a download of it still under way stops, and its part
//! is dropped. Never while the model runs or is the trial's candidate: the
//! kernel fills a running model straight from the blob's blocks.

use crate::Sha256;
use alloc::string::String;

/// What holds a model, as `updated` sees it when asked to remove one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Holds {
    /// The model `llm` reports loaded.
    pub running: Option<Sha256>,
    /// The candidate a pending trial switches to at the next restart.
    pub candidate: Option<Sha256>,
    /// The model an import from the attached disk is writing.
    pub importing: Option<Sha256>,
    /// The model a download is fetching.
    pub downloading: Option<Sha256>,
}

/// What a remove does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remove {
    /// Stop the download of it and drop what it fetched.
    Download,
    /// Take its blob out of the store.
    Blob,
}

/// What removing `sha256` does under `holds`, or why it may not, in words the
/// shell says as they are.
pub fn decide(sha256: &Sha256, holds: &Holds) -> Result<Remove, String> {
    let is = |held: Option<Sha256>| held.as_ref() == Some(sha256);
    if is(holds.running) {
        return Err(String::from(
            "it is the model running; use another model first",
        ));
    }
    if is(holds.candidate) {
        return Err(String::from(
            "it is on trial for the next restart; undo that switch first",
        ));
    }
    if is(holds.importing) {
        return Err(String::from(
            "it is still being imported; ask again once the import ends",
        ));
    }
    match is(holds.downloading) {
        true => Ok(Remove::Download),
        false => Ok(Remove::Blob),
    }
}
