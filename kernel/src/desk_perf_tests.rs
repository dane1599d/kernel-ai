//! `test=desk-perf` (P3's close, plan section 5): S05-1's budgets as figures
//! from the real `desktop` at the size `display=` asks for, with `deskdemo`'s
//! two 800x600 windows. `desktop` times its own frames (`perf=on`); this
//! drives it:
//! - `perf scenes`: (a) full composites, (b) the top window dragged 200 px,
//!   (c) a 160x45 terminal page warm and cold;
//! - (e): Ctrl+Space, then [`KEYS`] letters typed in the panel, each timed
//!   from its input event to the flush that showed it;
//! - (f): (b) again with one `work` hog per CPU under `classes`, as S05-1's
//!   scene (f) ran;
//! - (g): a Super tap opens Start beside the pane (e) opened, then (a)'s
//!   full composites with both open (Kai Desk PR 10).
//!
//! A gate under KVM: each miss is one `OVER BUDGET <figure> <value> ms
//! (budget ...)` line. Shared runners are noisy, so it is best of three
//! ([`kabi::perf`]): a scene with a miss is measured again in this boot, a
//! figure over in two runs a third time, every run's lines are logged, and
//! only a figure over its budget in all of them fails the case. Each budget
//! is for a runner of baseline speed, so the case judges a figure against its
//! budget times the runner's measured slowness (below), never less than the
//! budget ([`kabi::perf::misses_scaled`]). `desktop` logs the slowest frames of
//! each figure that misses (`the 8 slowest frames (1/2): #n ms, ...`), where a
//! steady cost shows as a run of them and a stall as a few far apart;
//! `desktop`'s own `OVER` lines use the bare budgets. Before a second run the
//! reference is timed again and both runs are judged by the larger median.
//! (g) is also judged against (a) from the same boot: its budget is the larger
//! of that and [`kabi::perf::COMPOSITE_TIMES_HUNDREDTHS`] of (a)'s p50
//! ([`kabi::perf::composite_budget`]), since the reference undersells the
//! composite on some CPU classes.
//! Under TCG the figures are recorded and not judged, with fewer frames.
//!
//! First `desktop` times `kabi::perf`'s fixed reference work. Under KVM a
//! median over 3.5 times its recorded baseline is a runner too slow for any
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
/// A Super tap: Left Super down and up.
const SUPER: [u8; 4] = [0xE0, 0x5B, 0xE0, 0xDB];

/// How long killed hogs get to be reaped before (f) is measured again.
const REAP_MS: u64 = 10_000;

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
                let _ = stop_hogs(&hogs);
                return Err("desk-perf: cannot start the hogs");
            }
        }
    }
    let classes = sched::spec::PolicySpec::new(
        sched::spec::PolicyKind::Classes,
        sched::spec::Preset::Gentle,
    );
    if crate::sched_control::install(classes, hogs.clone()).is_err() {
        let _ = stop_hogs(&hogs);
        return Err("desk-perf: cannot install classes");
    }
    log!("desk-perf: (f) {cpus} hogs under classes");
    Ok(hogs)
}

/// Kills the hogs and waits, up to [`REAP_MS`], until each is gone from the
/// process table: a hog still exiting holds its name, so `hog-<i>` cannot be
/// started again until then (Busy).
fn stop_hogs(hogs: &[u64]) -> Result<(), &'static str> {
    for &pid in hogs {
        let _ = process::kill(pid, ExitReason::Killed);
    }
    let _ = crate::sched_control::set_policy(sched::spec::PolicySpec::DEFAULT);
    let live = || {
        hogs.iter()
            .filter(|&&pid| process::slot_of_pid(pid).is_some())
            .count()
    };
    for _ in 0..REAP_MS / 10 {
        if live() == 0 {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    let left = live();
    if left == 0 {
        return Ok(());
    }
    log!(
        "desk-perf: {left} of {} killed hogs still in the process table after {REAP_MS} ms",
        hogs.len()
    );
    Err("desk-perf: the killed hogs were not reaped")
}

/// The scale `reference` gives the budgets, as `1.8`.
fn scale(reference: u64) -> String {
    kabi::perf::times_baseline(
        reference.clamp(kabi::perf::REFERENCE_BASELINE_NS, kabi::perf::SCALE_CAP_NS),
    )
}

/// The figures over their budget scaled for the runner (`reference`, the
/// reference median), each `desktop` miss within it logged as such. Each
/// figure is also logged with the ratio and itself over the ratio, so main's
/// runs show how well the reference predicts it.
fn judged<'a>(
    what: &str,
    run: &str,
    reply: &'a str,
    reference: u64,
    composite: Option<u64>,
) -> Vec<&'a str> {
    use kabi::perf::{hundredths_text, per_baseline, ratio_hundredths};
    for figure in kabi::perf::figures(reply) {
        if let Some(a) = composite.filter(|_| figure.name.starts_with(kabi::perf::COMPOSITE_FIGURE))
        {
            log!(
                "desk-perf: {what}, {run}: {}: judged at {} ms, the larger of {} ms for the runner and {} times (a)'s p50 {} ms (at most 1.15 times the first)",
                figure.name,
                hundredths_text(kabi::perf::composite_budget(figure.budget, reference, a)),
                hundredths_text(kabi::perf::scaled_budget(figure.budget, reference)),
                hundredths_text(kabi::perf::COMPOSITE_TIMES_HUNDREDTHS),
                hundredths_text(a)
            );
        }
        log!(
            "desk-perf: ratio: {what}, {run}: {}: {} {} ms at {}x, {} ms per baseline",
            figure.name,
            figure.label,
            hundredths_text(figure.value),
            hundredths_text(ratio_hundredths(reference)),
            hundredths_text(per_baseline(figure.value, reference))
        );
    }
    let over = kabi::perf::misses_judged(reply, reference, composite);
    for miss in kabi::perf::misses(reply) {
        if !over.contains(&miss) {
            match composite.filter(|_| miss.starts_with(kabi::perf::COMPOSITE_FIGURE)) {
                Some(_) => log!("desk-perf: {what}, {run}: within its judged budget: {miss}"),
                None => log!(
                    "desk-perf: {what}, {run}: within budget scaled {}x for the runner: {miss}",
                    scale(reference)
                ),
            }
        }
    }
    for miss in &over {
        log!("desk-perf: {what}, {run}: over budget: {miss}");
    }
    over
}

/// One scene's figures, best of three when `gated`: a run with a miss (against
/// the scaled budgets) is run once more, and a figure over budget in both is
/// run a third time; those over in all three are returned. Before the second
/// run the reference work is timed again, and both runs are judged by the
/// larger median. `composite` is (a)'s p50, for (g).
fn scene(
    gated: bool,
    reference: u64,
    composite: Option<u64>,
    what: &str,
    mut run: impl FnMut() -> Result<Vec<u8>, &'static str>,
) -> Result<Vec<String>, &'static str> {
    let first = run()?;
    let first = String::from_utf8_lossy(&first);
    if !gated {
        return Ok(Vec::new());
    }
    let over = judged(what, "first run", &first, reference, composite);
    if over.is_empty() {
        return Ok(Vec::new());
    }
    log!("desk-perf: {what}: measured once more, best of three");
    let remeasured = reference_ns()?;
    let judging = kabi::perf::judging_reference(reference, remeasured);
    log!(
        "desk-perf: {what}: the reference work again: {remeasured} ns; both runs judged at {}x",
        scale(judging)
    );
    let over = match judging == reference {
        true => over,
        false => judged(what, "first run, judged again", &first, judging, composite),
    };
    if over.is_empty() {
        return Ok(Vec::new());
    }
    let second = run()?;
    let second = String::from_utf8_lossy(&second);
    let again = judged(what, "second run", &second, judging, composite);
    let over = kabi::perf::still_over(&over, &again);
    if over.is_empty() {
        return Ok(Vec::new());
    }
    // Two misses in a row can both be a stall on the runner (main's runs show
    // a few frames of 25 to 50 ms, and with n=300 four of them make the p99),
    // where a real slowdown moves every run. A third run settles it; the
    // budget is the same.
    log!("desk-perf: {what}: over in two runs, measured a third time, best of three");
    let remeasured = reference_ns()?;
    let judging = kabi::perf::judging_reference(judging, remeasured);
    log!(
        "desk-perf: {what}: the reference work again: {remeasured} ns; judged at {}x",
        scale(judging)
    );
    let third = run()?;
    let third = String::from_utf8_lossy(&third);
    let last = judged(what, "third run", &third, judging, composite);
    Ok(kabi::perf::still_over(&over, &last)
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

/// (f): the drag again, under load. The hogs are reaped before it returns,
/// so a second run can start `hog-1..` afresh; a reap that runs out of time
/// fails the case only when it is `gated`, since only then is there a second
/// run.
fn loaded_drag(gated: bool) -> impl FnMut() -> Result<Vec<u8>, &'static str> {
    move || {
        let hogs = start_hogs()?;
        threads::sleep_ms(500);
        let loaded = perf("perf drag");
        if let Err(error) = stop_hogs(&hogs) {
            if gated {
                return Err(error);
            }
            log!("desk-perf: (f): not gated, so going on");
        }
        loaded
    }
}

/// (g): full composites with Start and the pane open; Start opens on the
/// first run only, and stays open for a second.
fn open_scene(frames: usize) -> impl FnMut() -> Result<Vec<u8>, &'static str> {
    let mut opened = false;
    move || {
        if !core::mem::replace(&mut opened, true) {
            crate::claim_tests::type_keys(&SUPER);
            threads::sleep_ms(SETTLE_MS);
        }
        perf(&format!("perf open {frames}"))
    }
}

/// The reference work's median, timed by `desktop` now.
fn reference_ns() -> Result<u64, &'static str> {
    let reply = perf("perf reference")?;
    kabi::perf::reference_ns(&String::from_utf8_lossy(&reply))
        .ok_or("desk-perf: desktop's reference line did not parse")
}

/// The reference work's median, and whether it is a runner too slow to judge.
fn too_slow() -> Result<(u64, bool), &'static str> {
    let p50 = reference_ns()?;
    let slow = kabi::perf::too_slow(p50);
    log!(
        "desk-perf: the reference work's median is {p50} ns, baseline {} ns: {}",
        kabi::perf::REFERENCE_BASELINE_NS,
        match slow {
            true => "over 3.5 times it",
            false => "within 3.5 times it",
        }
    );
    Ok((p50, slow))
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop and deskdemo", || {
        desktop_shows() && !process::named("deskdemo").is_empty()
    })?;
    threads::sleep_ms(SETTLE_MS);
    let kvm = under_kvm();
    let (reference, slow) = too_slow()?;
    let gated = kvm && !slow;
    if gated {
        log!(
            "desk-perf: budgets scaled {}x for the runner (the reference's median over its baseline, at least 1)",
            scale(reference)
        );
    }
    let frames = if kvm { KVM_FRAMES } else { TCG_FRAMES };
    let request = format!("perf scenes {frames}");
    // (a)'s p50, the larger over its runs: what (g) is also judged against.
    let mut composite = None::<u64>;
    let scenes = || {
        let reply = perf(&request)?;
        let p50 = kabi::perf::p50(
            &String::from_utf8_lossy(&reply),
            kabi::perf::COMPOSITE_REFERENCE,
        );
        composite = composite.max(p50);
        Ok(reply)
    };
    let mut misses = scene(gated, reference, None, "(a)-(c)", scenes)?;
    crate::claim_tests::type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    misses.extend(scene(gated, reference, None, "(e)", keys)?);
    misses.extend(scene(gated, reference, None, "(f)", loaded_drag(gated))?);
    misses.extend(scene(
        gated,
        reference,
        composite,
        "(g)",
        open_scene(frames),
    )?);
    log!("desk-perf: the figures are in");
    if !kvm {
        log!("desk-perf: recorded without KVM, not judged");
        return Ok(());
    }
    if slow {
        log!("desk-perf: runner too slow: figures reported, not gated");
        // xtask repeats it as a GitHub warning, so the run's summary shows it.
        log!(
            "ci-warning: desk-perf ungated (runner {}x slow)",
            kabi::perf::times_baseline(reference)
        );
        return Ok(());
    }
    for miss in &misses {
        log!("desk-perf: OVER BUDGET in every run: {miss}");
    }
    log!(
        "desk-perf: judged under KVM: {} figures over budget in every run",
        misses.len()
    );
    match misses.is_empty() {
        true => Ok(()),
        false => Err("desk-perf: a figure is over its budget under KVM"),
    }
}
