//! Updates over the network (P5 task 10, the host half; ADR 0008 sections 4, 7 and
//! 12): `updated` streams the next release into the idle slot, and `:reinstall`
//! in slot R fetches one.
//!
//! The release is a TUF target, `slot.img`, with a signed sibling target
//! `slot.img.chunks` holding its `kai-chunks 1` list. The signed metadata, read
//! through `kaituf` from the update repository ([`kaipkg::online`]'s hosts), is the
//! only thing trusted; the network is not.
//!
//! - [`plan::refresh`] and [`plan::plan`]: the metadata is refreshed, and the
//!   release it names must be newer than every release this machine has committed
//!   (anti-rollback), fit the idle slot, and not be staged already. The job's
//!   chunk list must be the target's own list, for the target's own bytes.
//! - [`slot::SlotPartial`]: the idle slot as `kainet`'s download writes it. A chunk
//!   is written only after it matches its signed hash, a checkpoint says how many
//!   are written (durably, after the slot is flushed), and the whole slot is read
//!   back and hashed before the release counts as written.
//! - [`stage::stage`]: the slot is marked Updating first, so a half-written slot
//!   never boots, then streamed. A cut, a timeout or a 429 stops with
//!   [`stage::Stop::Retry`], the checkpoint kept, and the next call resumes at the
//!   first unchecked chunk. A bad chunk, a bad whole, a short chunk, a refused
//!   certificate or redirect, or any other answer stops with [`stage::Stop::Bad`]:
//!   the slot stays unbootable and nothing after it is written. Only a release
//!   that checks out end to end becomes a trial slot ([`bootslot::stage_end`]),
//!   and only if the signed metadata, refreshed again just before
//!   ([`plan::Job::still_signed`]), still names it: a long download can span a newer
//!   release, a pulled one or metadata that has expired.
//! - [`net::Over`]: the transport, over `kainet`'s HTTPS, held to the repository's
//!   hosts on 443 for every hop of a redirect.
//!
//! What is not here: the `SLOT_WRITE` and kaifs implementations of the traits, the
//! update snapshot, the restart, DNS through RESOLVE and the `NetDest`. They are
//! `updated`'s, with the kernel's.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod net;
pub mod plan;
pub mod slot;
pub mod stage;

#[cfg(test)]
mod tests;
