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
//! A gate under KVM: each miss is one `OVER BUDGET <figure> <value> ms
//! (budget ...)` line. Shared runners are noisy, so it is best of two
//! ([`kabi::perf`]): a scene with a miss is measured once more in this boot,
//! both runs' lines are logged, and only a figure over its budget in both
//! fails the case. Under TCG the figures are recorded and not judged, with
//! fewer frames.
//!
//! First `desktop` times `kabi::perf`'s fixed reference work. Under KVM a
//! median over twice its recorded baseline is a runner too slow for any
//! budget: every figure is still measured and logged, once each, and the case
//! passes as `runner too slow: figures reported, not gated`.

use crate::desk_tests::{desktop_shows, wait};
use crate::process::{self, SpawnRequest};
use crate::{ipc, log, threads};
use alloc::format;
use alloc::string::String;
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

/// One scene's figures, best of two when `gated`: a run with a miss is run
/// once more, and the figures over budget in both are returned.
fn scene(
    gated: bool,
    what: &str,
    mut run: impl FnMut() -> Result<Vec<u8>, &'static str>,
) -> Result<Vec<String>, &'static str> {
    let first = run()?;
    let first = String::from_utf8_lossy(&first);
    let over = kabi::perf::misses(&first);
    if !gated || over.is_empty() {
        return Ok(Vec::new());
    }
    for miss in &over {
        log!("desk-perf: {what}, first run: over budget: {miss}");
    }
    log!("desk-perf: {what}: measured once more, best of two");
    let second = run()?;
    let second = String::from_utf8_lossy(&second);
    let again = kabi::perf::misses(&second);
    for miss in &again {
        log!("desk-perf: {what}, second run: over budget: {miss}");
    }
    Ok(kabi::perf::still_over(&over, &again)
        .into_iter()
        .map(String::from)
        .collect())
}

/// (e): the panel takes the letters, one damaged line each.
fn keys() -> Result<Vec<u8>, &'static str> {
    perf("perf forget")?;
    for _ in 0..KEYS {
        crate::claim_tests::type_keys(&LETTER);
        threads::sleep_ms(KEY_GAP_MS);
    }
    threads::sleep_ms(SETTLE_MS);
    perf("perf keys")
}

/// (f): the drag again, under load.
fn loaded_drag() -> Result<Vec<u8>, &'static str> {
    let hogs = start_hogs()?;
    threads::sleep_ms(500);
    let loaded = perf("perf drag");
    stop_hogs(&hogs);
    loaded
}

/// Whether the runner is too slow to judge, from the reference work's median.
fn too_slow() -> Result<bool, &'static str> {
    let reply = perf("perf reference")?;
    let p50 = kabi::perf::reference_ns(&String::from_utf8_lossy(&reply))
        .ok_or("desk-perf: desktop's reference line did not parse")?;
    let slow = kabi::perf::too_slow(p50);
    log!(
        "desk-perf: the reference work's median is {p50} ns, baseline {} ns: {}",
        kabi::perf::REFERENCE_BASELINE_NS,
        match slow {
            true => "over twice it",
            false => "within twice it",
        }
    );
    Ok(slow)
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop and deskdemo", || {
        desktop_shows() && !process::named("deskdemo").is_empty()
    })?;
    threads::sleep_ms(SETTLE_MS);
    let kvm = under_kvm();
    let slow = too_slow()?;
    let gated = kvm && !slow;
    let frames = if kvm { KVM_FRAMES } else { TCG_FRAMES };
    let request = format!("perf scenes {frames}");
    let mut misses = scene(gated, "(a)-(c)", || perf(&request))?;
    crate::claim_tests::type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    misses.extend(scene(gated, "(e)", keys)?);
    misses.extend(scene(gated, "(f)", loaded_drag)?);
    log!("desk-perf: the figures are in");
    if !kvm {
        log!("desk-perf: recorded without KVM, not judged");
        return Ok(());
    }
    if slow {
        log!("desk-perf: runner too slow: figures reported, not gated");
        return Ok(());
    }
    for miss in &misses {
        log!("desk-perf: OVER BUDGET in both runs: {miss}");
    }
    log!(
        "desk-perf: judged under KVM: {} figures over budget in both runs",
        misses.len()
    );
    match misses.is_empty() {
        true => Ok(()),
        false => Err("desk-perf: a figure is over its budget under KVM"),
    }
}
