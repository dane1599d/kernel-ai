//! `test=desk-perf` (P3's close, plan section 5): S05-1's budgets as figures
//! from the real `desktop` at the size `display=` asks for, with `deskdemo`'s
//! two 800x600 windows. `desktop` times its own frames (`perf=on`); this
//! drives it:
//! - `perf scenes`: (a) full composites, (b) the top window dragged 200 px,
//!   (c) a 160x45 terminal page warm and cold;
//! - (e): Ctrl+Space, then [`KEYS`] letters typed in the panel, each timed
//!   from its input event to the flush that showed it;
//! - (f): (b) again with one `work` hog per CPU under `classes`, as S05-1's
//!   scene (f) ran.
//!
//! Gated under KVM only: a line over its budget fails the case there. Under
//! TCG the figures are recorded and not judged, with fewer frames.

use crate::desk_tests::{desktop_shows, wait};
use crate::process::{self, SpawnRequest};
use crate::{ipc, log, threads};
use alloc::format;
use alloc::vec::Vec;
use kabi::{ExitReason, Limits};

/// The plan lines this case adds: room for a 1920x1080 frame and the text
/// page beside it, and bursts as long as a scene.
pub const PLAN: &str = "desktop caps=display+input+serve+ask pages=16384 stack_pages=512 burst_ms=60000 perf=on\n\
     deskdemo uses=desktop caps=call pages=2048 size=800x600";
/// Frames for (a) and pages for (c) warm: S05-1's 300 under KVM.
const KVM_FRAMES: usize = 300;
const TCG_FRAMES: usize = 30;
/// Letters typed for (e).
const KEYS: usize = 50;
/// Between them: a key a frame or two apart, as a fast typist's.
const KEY_GAP_MS: u64 = 60;
/// A scene under TCG takes minutes.
const SCENES_MS: u64 = 900_000;
/// Time for `deskdemo` to open its windows, and for keys to land.
const SETTLE_MS: u64 = 3_000;
/// Ctrl down, Space down and up, Ctrl up; `a` down and up (scan code set 1).
const CTRL_SPACE: [u8; 4] = [0x1D, 0x39, 0xB9, 0x9D];
const LETTER: [u8; 2] = [0x1E, 0x9E];

/// M10's `work` hogs: CPU-bound, never blocking.
const HOG_LIMITS: Limits = Limits {
    max_pages: 1024,
    max_burst_ms: 600_000,
    caps: 0,
    stack_pages: 0,
};

fn under_kvm() -> bool {
    cpufeat::hypervisor_signature().is_some_and(|s| cpufeat::hypervisor_name(&s) == "KVM")
}

/// `desktop`'s answer to `request`, each line logged by `desktop` itself.
fn perf(request: &str) -> Result<Vec<u8>, &'static str> {
    let reply = ipc::call("desktop", request.as_bytes(), SCENES_MS)
        .map_err(|_| "desk-perf: desktop did not answer")?;
    if reply.starts_with(b"err") {
        log!(
            "desk-perf: {request}: {}",
            core::str::from_utf8(&reply).unwrap_or("?")
        );
        return Err("desk-perf: desktop refused the request");
    }
    Ok(reply)
}

/// One hog per CPU, at most six, labelled background under `classes`.
fn start_hogs() -> Result<Vec<u64>, &'static str> {
    let cpus = crate::smp::online().clamp(1, 6);
    let mut hogs = Vec::new();
    for i in 1..=cpus {
        let pid = process::spawn(&SpawnRequest {
            name: &format!("hog-{i}"),
            program: "work",
            args: format!("mix=B index={i} seed=1").as_bytes(),
            limits: HOG_LIMITS,
            spawner: 0,
            supervisor: false,
            uses: &[],
        });
        match pid {
            Ok(pid) => hogs.push(pid),
            Err(error) => {
                log!("desk-perf: cannot start hog-{i}: {error:?}");
                stop_hogs(&hogs);
                return Err("desk-perf: cannot start the hogs");
            }
        }
    }
    let classes = sched::spec::PolicySpec::new(
        sched::spec::PolicyKind::Classes,
        sched::spec::Preset::Gentle,
    );
    if crate::sched_control::install(classes, hogs.clone()).is_err() {
        stop_hogs(&hogs);
        return Err("desk-perf: cannot install classes");
    }
    log!("desk-perf: (f) {cpus} hogs under classes");
    Ok(hogs)
}

fn stop_hogs(hogs: &[u64]) {
    for &pid in hogs {
        let _ = process::kill(pid, ExitReason::Killed);
    }
    let _ = crate::sched_control::set_policy(sched::spec::PolicySpec::DEFAULT);
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop and deskdemo", || {
        desktop_shows() && !process::named("deskdemo").is_empty()
    })?;
    threads::sleep_ms(SETTLE_MS);
    let gated = under_kvm();
    let frames = if gated { KVM_FRAMES } else { TCG_FRAMES };
    let mut lines = perf(&format!("perf scenes {frames}"))?;
    // (e): the panel takes the letters, one damaged line each.
    crate::claim_tests::type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    perf("perf forget")?;
    for _ in 0..KEYS {
        crate::claim_tests::type_keys(&LETTER);
        threads::sleep_ms(KEY_GAP_MS);
    }
    threads::sleep_ms(SETTLE_MS);
    lines.push(b'\n');
    lines.extend(perf("perf keys")?);
    // (f): the drag again, under load.
    let hogs = start_hogs()?;
    threads::sleep_ms(500);
    let loaded = perf("perf drag");
    stop_hogs(&hogs);
    lines.push(b'\n');
    lines.extend(loaded?);
    log!("desk-perf: the figures are in");
    let over = lines.windows(6).any(|w| w == b": OVER");
    match (gated, over) {
        (true, true) => Err("desk-perf: a figure is over its budget under KVM"),
        (true, false) => {
            log!("desk-perf: gated under KVM, and every figure is within its budget");
            Ok(())
        }
        (false, _) => {
            log!("desk-perf: recorded without KVM, not gated");
            Ok(())
        }
    }
}
