//! **Interim** (S10-3; this file and its line in `AREAS` go at P2 task 10, when
//! llm's KV files move to `/system/models/cache/`): MODEL_KV_LOAD and
//! MODEL_KV_SAVE, llm's KV slots kept in the `kai-cache` partition. Both need
//! `cap::MODEL`; `model::kv_io` checks the caller is the model's holder and every
//! bound.

use super::need;
use kabi::{Error, call, cap};

/// The calls this area serves.
const CALLS: &[u64] = &[call::MODEL_KV_LOAD, call::MODEL_KV_SAVE];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| {
        need(slot, cap::MODEL)?;
        let flush = match (number, a[4]) {
            (call::MODEL_KV_LOAD, _) => None,
            (_, 0) => Some(false),
            (_, 1) => Some(true),
            _ => return Err(Error::Invalid),
        };
        crate::model::kv_io(slot, a[0], a[1], a[2], a[3], flush)
    })
}
