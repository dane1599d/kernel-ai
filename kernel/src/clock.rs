//! The date (ADR 0008 §7, "Clock"): Limine's date at boot plus the time since
//! boot, in seconds since 1970 UTC. TUF's expiry checks in `updated` read it
//! through TRUST_TIME; nothing else in the kernel needs the date yet.

use limine::request::DateAtBootRequest;

#[used]
#[unsafe(link_section = ".requests")]
static DATE: DateAtBootRequest = DateAtBootRequest::new();

/// Seconds since 1970 UTC now, or `None` when the firmware gave no date.
pub fn unix_seconds() -> Option<u64> {
    let at_boot = u64::try_from(DATE.response()?.timestamp).ok()?;
    Some(at_boot + crate::hpet::nanos() / 1_000_000_000)
}
