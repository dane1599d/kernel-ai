//! `test=net-stream` (P5 task 2): TCP streams on a `NetDest` handle, to
//! xtask's echo server, from `netcheck`'s stream modes ([`stream_check`]).
//! Moved out of `net_tests.rs`, whose setup helpers it shares.

use crate::fpu_tests::wait_exit;
use crate::net::dest::Meter;
use crate::net_tests::{NETCHECK_LIMITS, SETUP_WAIT_MS, netcheck, netdest, wait_for};
use crate::process::{self, SpawnRequest};
use crate::{log, net, threads};
use core::net::Ipv4Addr;
use kabi::ExitReason;

/// `test=net-stream` (P5 task 2): TCP streams on a `NetDest` handle, to xtask's
/// echo server (`echo=`), from `netcheck`'s stream modes.
/// - A stream and a bulk one echo 8 KiB and a line; a send over 8 KiB is
///   TooBig; a port the handle does not name is refused; the 17th stream of a
///   process is Busy; each closes, and none is left open.
/// - A process killed while it holds three streams leaves none open.
/// - Revoking the grant a `NetDest` was given under stops its streams, its
///   narrowing and itself ([`revoke_check`]).
/// - A refused HANDLE_REPLACE on a stream closes the stream ([`replace_check`]).
/// - An agent's `NetDest` past its day's bytes is refused a receive, and the
///   echo it would have read is not counted ([`budget_check`]).
/// - Past its day's bytes, a receive on a stream that ended still gives 0
///   ([`budget_end_check`]).
/// - A connect never takes the local port of a stream still open, even once
///   the port counter has come round to it ([`port_check`]).
pub fn stream_check(cmdline: &str) -> Result<(), &'static str> {
    let (server, port) = cmdline
        .split_whitespace()
        .find_map(|w| net::parse_endpoint(w.strip_prefix("echo=")?))
        .ok_or("net-stream: no echo= on the command line")?;
    wait_for(|s| s.address.is_some())?;
    let spec = alloc::format!("proto:tcp,addrs:{server},ports:{port}");
    let (_, entry) = netdest(&spec)?;
    netcheck(
        "netstream-a",
        &alloc::format!("stream=echo port={port}"),
        Some(entry),
    )?;
    if net::stream::count(0).1 != 0 {
        return Err("net-stream: a closed stream was left open");
    }
    log!("net-stream: streams echoed through the handle, kept to its port and to 16, and closed");
    match net::interrupts_taken() {
        Some(taken) if taken > 0 => log!("net-stream: the card raised {taken} receive interrupts"),
        Some(_) => log!("net-stream: receive interrupts are on, but none came"),
        None => log!("net-stream: the card was polled, with no receive interrupts"),
    }
    kicks_check()?;
    let (_, entry) = netdest(&spec)?;
    let args = alloc::format!("stream=hold port={port}");
    let request = SpawnRequest {
        name: "netstream-b",
        program: "netcheck",
        args: args.as_bytes(),
        limits: NETCHECK_LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    let pid = process::spawn_with(&request, alloc::vec![entry], false)
        .map_err(|_| "net-stream: cannot start netcheck")?;
    if !comes_to_hold(pid, 3) {
        return Err("net-stream: netcheck never held 3 streams");
    }
    process::kill(pid, ExitReason::Killed).map_err(|_| "net-stream: cannot kill netcheck")?;
    wait_exit(pid)?;
    if net::stream::count(pid) != (0, 0) {
        return Err("net-stream: a killed process's streams stayed open");
    }
    log!("net-stream: a process killed with 3 streams open left none");
    revoke_check(&spec, port)?;
    replace_check(&spec, port)?;
    budget_check(&spec, port)?;
    budget_end_check(&spec, port)?;
    port_check(server, port)
}

/// The card is kicked for receive buffers only when it asked to hear of them
/// (G67): with 256 buffers and slirp's few frames in flight it never runs dry,
/// so after the echoes far fewer kicks than frames. A kick for each poll that
/// gave a buffer back was about one a frame.
fn kicks_check() -> Result<(), &'static str> {
    let (frames, rx, tx) = net::card_counts();
    log!(
        "net-stream: {frames} frames received; the card was kicked {rx} times for receive buffers, {tx} for frames to send"
    );
    if frames < 32 {
        return Err("net-stream: under 32 frames came, too few to count the kicks");
    }
    if rx * 8 > frames {
        return Err(
            "net-stream: the card was kicked for receive buffers more than once in 8 frames",
        );
    }
    log!("net-stream: the card was kicked for receive buffers under once in 8 frames");
    Ok(())
}

/// Whether the process `pid` comes to hold `n` open streams within
/// [`SETUP_WAIT_MS`].
fn comes_to_hold(pid: u64, n: usize) -> bool {
    for _ in 0..SETUP_WAIT_MS / 10 {
        if net::stream::count(pid).0 == n {
            return true;
        }
        threads::sleep_ms(10);
    }
    false
}

/// Bug hunt 2's H2-18: `netcheck` (`stream=revoke`) holds a `NetDest` given
/// under a grant node, a narrowing of it, and 3 streams on the two. Revoking
/// the node aborts the streams at once, before `netcheck` looks; from then on
/// a poll waiting on one, every use of them, of the `NetDest` and of the
/// narrowing answers Revoked, and the streams' handles still close.
fn revoke_check(spec: &str, port: u16) -> Result<(), &'static str> {
    use crate::handle::grant as grants;
    use captable::grant::{Life, SYSTEM};
    let node = grants::create(grants::root(SYSTEM), 0, Life::Request, b"net-stream")
        .map_err(|_| "net-stream: cannot make a grant node")?;
    let (_, entry) = netdest(spec)?;
    let entry = captable::Entry {
        grant: Some(node),
        ..entry
    };
    let args = alloc::format!("stream=revoke port={port}");
    let request = SpawnRequest {
        name: "netstream-c",
        program: "netcheck",
        args: args.as_bytes(),
        limits: NETCHECK_LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    let Ok(pid) = process::spawn_with(&request, alloc::vec![entry], false) else {
        let _ = grants::revoke(node);
        return Err("net-stream: cannot start netcheck");
    };
    // The third stream opens once the first two have echoed.
    let held = comes_to_hold(pid, 3);
    let revoked = grants::revoke(node);
    // Nothing `netcheck` does can open one meanwhile: every connect on the
    // NetDest or its narrowing now answers Revoked.
    let open = net::stream::count(pid).0;
    if !held || revoked.is_err() {
        let _ = process::kill(pid, ExitReason::Killed);
        let _ = wait_exit(pid);
        return Err("net-stream: netcheck never held 3 streams on its granted NetDest");
    }
    let exit = wait_exit(pid)?;
    if open != 0 {
        return Err("net-stream: a revoke left the streams on its grant open");
    }
    if exit != (ExitReason::Exited, 0) {
        return Err(
            "net-stream: a revoked NetDest, its narrowing or a stream on them still worked",
        );
    }
    if net::stream::count(pid) != (0, 0) || net::stream::revoked_places(pid) != 0 {
        return Err("net-stream: a revoked stream outlived its process");
    }
    log!(
        "net-stream: a revoke aborted the 3 streams on a granted NetDest and its narrowing at once; each use after answered Revoked"
    );
    Ok(())
}

/// Bug hunt 2's H2-17: a refused HANDLE_REPLACE closes a stream's handle, and
/// with it the stream, as HANDLE_CLOSE does: `netcheck` (`stream=replace`)
/// opens 17 in turn, each refused, and never meets the limit of 16.
fn replace_check(spec: &str, port: u16) -> Result<(), &'static str> {
    let (_, entry) = netdest(spec)?;
    netcheck(
        "netstream-d",
        &alloc::format!("stream=replace port={port}"),
        Some(entry),
    )?;
    log!(
        "net-stream: a refused HANDLE_REPLACE closed the stream with its handle: 17 in a row never met the limit of 16"
    );
    Ok(())
}

/// A `NetDest` record for `spec` that is an agent's, with a 1 MiB day of
/// which only `left` bytes remain: its meter, and a handle's entry for it.
fn metered_dest(spec: &str, left: u64) -> Result<(Meter, captable::Entry), &'static str> {
    let meter = net::dest::meter(1);
    let today = net::dest::today();
    net::dest::with_meter(&meter, |m| m.try_send(netdest::meter::MIB - left, today))
        .map_err(|_| "net-stream: cannot spend the test meter's day")?;
    let dest = netdest::Dest::parse(spec).map_err(|_| "net-stream: a bad test destination")?;
    let live = |object| crate::handle::names(kabi::handle::Kind::NetDest, object);
    let object = net::dest::create_metered(dest, live, Some(meter.clone()))
        .ok_or("net-stream: no room for a NetDest record")?;
    let entry = captable::Entry::new(
        kabi::handle::Kind::NetDest,
        crate::handle::NETDEST_RIGHTS,
        object,
    );
    Ok((meter, entry))
}

/// Bug hunt 5's H5-18: `netcheck` (`stream=budget`) holds an agent's
/// `NetDest` with 8 KiB of its day left. Its 8 KiB send spends the day, and
/// the echo then stays unread: NET_RECV is refused, as a send and a connect
/// are, and the meter counts nothing past the budget.
fn budget_check(spec: &str, port: u16) -> Result<(), &'static str> {
    let (meter, entry) = metered_dest(spec, net::stream::MAX_CALL as u64)?;
    netcheck(
        "netstream-e",
        &alloc::format!("stream=budget port={port}"),
        Some(entry),
    )?;
    if net::dest::with_meter(&meter, |m| m.used()) != netdest::meter::MIB {
        return Err("net-stream: a refused receive was counted against the day");
    }
    log!(
        "net-stream: an agent's NetDest past its day's bytes was refused a receive, a send and a connect, and counted nothing past its budget"
    );
    Ok(())
}

/// #1433's review: `netcheck` (`stream=budget-end`) holds an agent's
/// `NetDest` with 8 bytes of its day left. It sends `bye\n`, which xtask's
/// echo server sends back and then closes; reading the echo spends the day
/// exactly (a receive is charged its whole cap before it reads and given back
/// the rest), and a receive on the ended stream then gives 0, not
/// Permission, so the app can tell the stream ended.
fn budget_end_check(spec: &str, port: u16) -> Result<(), &'static str> {
    let (meter, entry) = metered_dest(spec, 8)?;
    netcheck(
        "netstream-f",
        &alloc::format!("stream=budget-end port={port}"),
        Some(entry),
    )?;
    if net::dest::with_meter(&meter, |m| m.used()) != netdest::meter::MIB {
        return Err("net-stream: an 8-byte day's bye out and back did not spend it exactly");
    }
    log!("net-stream: past its day's bytes, a receive on an ended stream gave 0, not Permission");
    Ok(())
}

/// Bug hunt 5's H5-19: a connect never takes the local port of a stream
/// still open, even once the port counter has come round to it, so no new
/// stream to the same server takes a live one's four-tuple.
fn port_check(server: Ipv4Addr, port: u16) -> Result<(), &'static str> {
    let held = net::stream::connect(0, server, port, false, None, None)
        .map_err(|_| "net-stream: cannot open a stream to hold a port")?;
    let result = match net::stream::local_port(0, held) {
        None => Err("net-stream: a new stream has no local port"),
        Some(taken) => {
            // Where 16383 more connects would leave the counter.
            net::draw_next(taken);
            match net::stream::connect(0, server, port, false, None, None) {
                Err(_) => Err("net-stream: cannot open a second stream"),
                Ok(next) => {
                    let got = net::stream::local_port(0, next);
                    let _ = net::stream::close(0, next);
                    match got {
                        Some(got) if got != taken => Ok((taken, got)),
                        _ => Err("net-stream: a connect took the local port a live stream holds"),
                    }
                }
            }
        }
    };
    let _ = net::stream::close(0, held);
    let (taken, got) = result?;
    log!(
        "net-stream: with the port counter come round to a live stream's port {taken}, the next connect took {got}"
    );
    Ok(())
}
