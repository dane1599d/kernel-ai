//! What `updated` answers `models offers` and `models ask <id>` with (P5 task
//! 9): the signed `models` role's targets this machine may download, whether
//! each fits (`kai-offers 1`), and the one ask for one of them, which the shell
//! puts on its Allow / Deny menu. `modelchan::fetch` decides; this reads what
//! it decides by: the guest's memory (METRICS' `ram_frames`), the store
//! volume's free blocks (`Df`), what `.partial/` holds of each target, and the
//! hosts models come from.

use crate::folders;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_models::{Sha256, Signed, Store};
use fsproto::{Reply, Request};
use kabi::handle::Handle;
use modelchan::fetch::{self, FIXTURE, HUGGING_FACE, Hosts, Machine};
use modelchan::offered;
use user::sys;

/// `fs`'s blocks, which `Df` counts.
const BLOCK: u64 = 4096;
/// A frame of memory, which METRICS counts.
const FRAME: u64 = 4096;

/// The hosts models come from: Hugging Face, or, in the kernel's
/// `model-download` case (`fetch=test`), the HTTPS mock's.
pub fn hosts(test: bool) -> &'static [Hosts] {
    match test {
        true => &[FIXTURE],
        false => &[HUGGING_FACE],
    }
}

/// What this machine has for a download, read when asked.
pub struct Facts {
    memory_mib: u64,
    free_bytes: u64,
    partials: Vec<(Sha256, u64)>,
}

impl Facts {
    /// The guest's memory, the store volume's free bytes, and the bytes in
    /// `.partial/` for each of `signed`'s targets.
    pub fn read(store: Handle, signed: &[Signed]) -> Result<Facts, String> {
        let metrics = sys::metrics().map_err(|e| format!("METRICS: {e:?}"))?;
        let memory_mib = fetch::guest_mib_of(metrics.ram_frames.saturating_mul(FRAME));
        let free_bytes = match folders::ask(store, &Request::Df)? {
            (Reply::Space { free, .. }, _) => free.saturating_mul(BLOCK),
            other => return Err(format!("the store's free space: {other:?}")),
        };
        let mut partials = Vec::new();
        for target in signed {
            if let Some(size) = folders::size(store, &fetch::partial_name(&target.sha256))? {
                partials.push((target.sha256, size));
            }
        }
        Ok(Facts {
            memory_mib,
            free_bytes,
            partials,
        })
    }

    pub fn machine<'a>(&'a self, store: &'a Store, hosts: &'a [Hosts]) -> Machine<'a> {
        Machine {
            memory_mib: self.memory_mib,
            free_bytes: self.free_bytes,
            store,
            partials: &self.partials,
            hosts,
        }
    }
}

/// `models offers`: `kai-offers 1`.
pub fn offers(signed: &[Signed], machine: &Machine<'_>) -> String {
    offered::encode(&fetch::offers(signed, machine))
}

/// `models ask <id>`: `ok <the menu's subject>` for a target that may be
/// downloaded now, or why not.
pub fn ask(signed: &[Signed], machine: &Machine<'_>, id: &str) -> Result<String, String> {
    let job = fetch::plan(signed, machine, id).map_err(|why| why.why())?;
    let target = signed
        .iter()
        .find(|t| t.id == job.id)
        .ok_or("the role changed")?;
    Ok(format!("ok {}", fetch::subject(target, &job.source)))
}
