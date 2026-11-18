//! `test=agent-net-budget` (P5 task 14): an agent `NetDest`'s bytes a day,
//! counted by the kernel on its record. `httpscheck` (`budget=URL`), with NET
//! and a handle to the mock's repository host metered at 1 MiB a day (as a
//! plan line's `net_mib=1` gives it), GETs a 48 KiB file over and over:
//! - every GET until the day's bytes run out gets its 200;
//! - then a send is refused before it leaves, the stream it was on is dropped,
//!   and a new connection on the same handle is refused too, Permission each;
//! - what it moved, both ways, stays within the day's MiB and one call more.

use crate::fpu_tests::wait_exit;
use crate::process::{self, SpawnRequest};
use crate::{log, net};
use kabi::{ExitReason, Limits, cap};
use netcore::fixture;

/// The MiB a day the handle may move.
const MIB_A_DAY: u32 = 1;
/// A TLS client's room, as `https-mock`'s.
const LIMITS: Limits = Limits {
    max_pages: 4096,
    max_burst_ms: 120_000,
    caps: cap::NET,
    stack_pages: 64,
};

/// A handle to the repository host on 443, its record metered at
/// [`MIB_A_DAY`].
fn metered() -> Result<captable::Entry, &'static str> {
    let spec = alloc::format!("proto:tcp,ports:443,hosts:{}", fixture::MOCK_REPO);
    let dest = netdest::Dest::parse(&spec).map_err(|_| "agent-net-budget: a bad destination")?;
    let live = |object| crate::handle::names(kabi::handle::Kind::NetDest, object);
    let meter = net::dest::meter(MIB_A_DAY);
    let object = net::dest::create_metered(dest, live, Some(meter))
        .ok_or("agent-net-budget: no room for a NetDest")?;
    Ok(captable::Entry::new(
        kabi::handle::Kind::NetDest,
        crate::handle::NETDEST_RIGHTS,
        object,
    ))
}

pub fn check() -> Result<(), &'static str> {
    crate::https_tests::wait_up()?;
    let args = alloc::format!("budget=https://{}/big mib={MIB_A_DAY}", fixture::MOCK_REPO);
    let request = SpawnRequest {
        name: "agent-net",
        program: "httpscheck",
        args: args.as_bytes(),
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    let pid = process::spawn_with(&request, alloc::vec![metered()?], false)
        .map_err(|_| "agent-net-budget: cannot start httpscheck")?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => {}
        _ => return Err("agent-net-budget: httpscheck did not see its day's bytes run out"),
    }
    log!(
        "agent-net-budget: the handle moved its {MIB_A_DAY} MiB a day, then a send and a new connection were refused"
    );
    Ok(())
}
