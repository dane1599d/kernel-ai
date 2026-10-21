//! M10a-3 self-test (`test=tune-revert workload=A`): kernel-owned trials on the
//! mix, with no LLM. Every outcome here is a revert or a consistent verdict, so
//! it holds under noisy TCG timing as well as KVM:
//! (a) round-robin is reverted (its effect is tens of ms against a few);
//! (b) a starving test policy, which SCHED_TRIAL never accepts, is reverted by
//!     the guardrail at the first tick past 100 ms of waiting, before the 200 ms
//!     watchdog, with every CPU back on the last good policy;
//! (c) `classes` with the build jobs labelled runs every window, and its verdict
//!     is `sched::judge` on its own windows;
//! (d) a person's policy switch during a trial cancels it and wins;
//! and no workload process is killed as a runaway meanwhile.

use crate::sched_control::trial::{self, Candidate, Request};
use crate::sched_control::{self, PROTECTED};
use crate::{log, process, threads};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use kabi::sched::{TrialStatus, reason, state};
use sched::judge::GUARDRAIL_WAIT_NS;
use sched::spec::{PolicyKind, PolicySpec, Preset};
use sched::{EnqueueReason, Interactive, SchedPolicy, Tid};

const MS: u64 = 1_000_000;

/// The thread the test policy never queues.
static VICTIM: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The victim and its companion spin while this is set.
static SPINNING: AtomicBool = AtomicBool::new(false);

/// Spins, always runnable, until the case is done.
fn spin(_: usize) {
    let mut counter = 0u64;
    while SPINNING.load(Ordering::Relaxed) {
        counter = core::hint::black_box(counter.wrapping_add(1));
    }
}

/// Interactive that silently drops the victim: the guardrail must catch it.
struct Starving(Interactive);

impl SchedPolicy for Starving {
    fn name(&self) -> &'static str {
        "starving (deliberately broken)"
    }
    fn enqueue(&mut self, tid: Tid, reason: EnqueueReason) {
        if tid != VICTIM.load(Ordering::Relaxed) {
            self.0.enqueue(tid, reason);
        }
    }
    fn dequeue(&mut self, tid: Tid) {
        self.0.dequeue(tid);
    }
    fn pick_next(&mut self) -> Option<Tid> {
        self.0.pick_next()
    }
    fn tick(&mut self, current: Tid, slice_ns: u64) -> bool {
        self.0.tick(current, slice_ns)
    }
    fn preempt_on_wake(&self, woken: Tid, current: Tid) -> bool {
        self.0.preempt_on_wake(woken, current)
    }
    fn reset(&mut self) {
        self.0.reset();
    }
}

fn starving() -> Box<dyn SchedPolicy> {
    Box::new(Starving(Interactive::new()))
}

/// Queues nothing and picks nothing: the Core ejects it at its first pick
/// with a thread runnable, on every CPU.
struct Refusing;

impl SchedPolicy for Refusing {
    fn name(&self) -> &'static str {
        "refusing (deliberately broken)"
    }
    fn enqueue(&mut self, _: Tid, _: EnqueueReason) {}
    fn dequeue(&mut self, _: Tid) {}
    fn pick_next(&mut self) -> Option<Tid> {
        None
    }
    fn tick(&mut self, _: Tid, _: u64) -> bool {
        true
    }
    fn reset(&mut self) {}
}

fn refusing() -> Box<dyn SchedPolicy> {
    Box::new(Refusing)
}

/// Every CPU runs `name`, installed and running, none on the fallback.
fn running_everywhere(name: &str) -> bool {
    (0..crate::smp::online()).all(|cpu| {
        threads::with_core(cpu, |core, _| {
            !core.on_fallback() && !core.in_trial() && core.policy_name() == name
        }) == Some(true)
    })
}

/// The pid of the live process named `name`, waiting up to 10 s for it.
pub fn pid_named(name: &str) -> Result<u64, &'static str> {
    for _ in 0..200 {
        let mut found = None;
        process::services(|pid, n| {
            if n.as_str() == name {
                found = Some(pid);
            }
        });
        if let Some(pid) = found {
            return Ok(pid);
        }
        threads::sleep_ms(50);
    }
    Err("tune: a workload process never started")
}

/// Waits for trial `id` to end.
pub fn finished(id: u64) -> Result<TrialStatus, &'static str> {
    for _ in 0..1_200 {
        let status = sched_control::status();
        if status.id == id && status.state != state::RUNNING {
            return Ok(status);
        }
        threads::sleep_ms(50);
    }
    Err("tune: a trial never ended")
}

fn run(
    candidate: Candidate,
    target: u64,
    background: Vec<u64>,
    window_ms: u64,
    pairs: usize,
) -> Result<u64, &'static str> {
    trial::start(Request {
        candidate,
        target_pid: target,
        background,
        window_ns: window_ms * MS,
        pairs,
        starter_pid: 0,
    })
    .map_err(|_| "tune: the kernel refused a trial")
}

fn spec(policy: PolicyKind, preset: Preset) -> Candidate {
    Candidate::Spec(PolicySpec::new(policy, preset))
}

pub fn revert_check() -> Result<(), &'static str> {
    let editor = pid_named("editor")?;
    let builds = ["build-1", "build-2", "build-3", "build-4"]
        .iter()
        .map(|name| pid_named(name))
        .collect::<Result<Vec<_>, _>>()?;
    for name in PROTECTED {
        // The service list leaves the supervisor out; the kernel knows its pid.
        let pid = match name {
            "supervisor" => process::supervision::supervisor_pid(),
            _ => pid_named(name)?,
        };
        if pid == 0 || sched_control::labelable(pid) {
            return Err("tune: a protected service could be labelled");
        }
    }
    threads::sleep_ms(1_000);
    let runaways = sched_control::runaways();

    // (a)
    let id = run(
        spec(PolicyKind::RoundRobin, Preset::Gentle),
        editor,
        Vec::new(),
        1_000,
        2,
    )?;
    let status = finished(id)?;
    log!(
        "tune-revert: (a) round-robin: state {} reason {}, editor p99 {} us -> {} us",
        status.state,
        status.reason,
        status.baseline_p99_ns / 1000,
        status.candidate_p99_ns / 1000
    );
    if status.state != state::REVERTED {
        return Err("tune: round-robin was kept");
    }

    // (b)
    // The victim spins on one CPU next to a companion that also spins, both
    // pinned: the starving policy always has the companion to pick, so it is
    // never ejected for picking nothing, and only the victim's wait grows. (With
    // the editor as the victim, a CPU where it was the only runnable thread
    // ejected the policy at once, which reverted, but by the ejection rule.)
    let cpu = crate::smp::online() - 1;
    SPINNING.store(true, Ordering::SeqCst);
    let victim = threads::spawn_pinned(cpu, "tune-victim", spin, 0)?;
    threads::spawn_pinned(cpu, "tune-companion", spin, 1)?;
    VICTIM.store(victim, Ordering::SeqCst);
    threads::sleep_ms(100);
    let ejections = threads::stats().1;
    let id = run(Candidate::Test(starving), editor, Vec::new(), 2_000, 2)?;
    let status = finished(id)?;
    let trip = crate::sched_control::guard::last_trip();
    VICTIM.store(usize::MAX, Ordering::SeqCst);
    SPINNING.store(false, Ordering::SeqCst);
    log!(
        "tune-revert: (b) starving thread {victim} on cpu {cpu}: reason {}, thread {}; waited {} us at the revert, {} us at the tick before; {} new ejections",
        status.reason,
        trip.tid,
        trip.wait_ns / 1000,
        trip.previous_wait_ns / 1000,
        threads::stats().1 - ejections
    );
    if status.reason != reason::GUARDRAIL || trip.tid != victim as u64 {
        return Err("tune: the guardrail did not revert the starving policy");
    }
    if trip.previous_wait_ns > GUARDRAIL_WAIT_NS || trip.wait_ns <= GUARDRAIL_WAIT_NS {
        return Err("tune: the guardrail did not revert at the first tick past 100 ms");
    }
    if threads::stats().1 != ejections {
        return Err("tune: the watchdog ejected before the guardrail reverted");
    }
    if !threads::installed_policy_everywhere("interactive") {
        return Err("tune: a CPU did not go back to the last good policy");
    }
    log!(
        "tune-revert: the guardrail reverted every CPU within 100 ms + 1 tick, before the watchdog"
    );

    // (b2) The other route: a candidate the watchdog ejects during the trial
    // is reverted to the last good policy on every CPU, not left on the
    // watchdog's round-robin.
    let ejections = threads::stats().1;
    let id = run(Candidate::Test(refusing), editor, Vec::new(), 1_000, 2)?;
    let status = finished(id)?;
    log!(
        "tune-revert: (b2) refusing: reason {}; {} new ejections",
        status.reason,
        threads::stats().1 - ejections
    );
    if status.reason != reason::EJECTED || threads::stats().1 == ejections {
        return Err("tune: an ejected candidate was not reverted for its ejection");
    }
    if !running_everywhere("interactive") {
        return Err("tune: after an ejection a CPU is not running the last good policy");
    }
    log!(
        "tune-revert: an ejection during the trial restored interactive on every CPU, not round-robin"
    );

    // (c)
    let id = run(
        spec(PolicyKind::Classes, Preset::Firm),
        editor,
        builds.clone(),
        1_000,
        3,
    )?;
    let status = finished(id)?;
    let pairs = sched_control::last_pairs();
    let verdict = sched::judge::judge(&pairs, 0);
    log!(
        "tune-revert: (c) classes/firm: state {} reason {} windows {}; editor p99 {} us -> {} us, background {} permille",
        status.state,
        status.reason,
        status.windows_done,
        status.baseline_p99_ns / 1000,
        status.candidate_p99_ns / 1000,
        status.background_permille
    );
    if pairs.len() != 3 || status.windows_done != 6 {
        return Err("tune: the classes trial did not run every window");
    }
    if verdict.kept != (status.state == state::KEPT)
        || verdict.baseline_p99_ns != status.baseline_p99_ns
        || verdict.candidate_p99_ns != status.candidate_p99_ns
    {
        return Err("tune: the printed verdict is not the judge's on the trial's windows");
    }

    // (d)
    sched_control::set_policy(PolicySpec::DEFAULT).map_err(|_| "tune: cannot reset")?;
    let id = run(
        spec(PolicyKind::Classes, Preset::Gentle),
        editor,
        builds,
        2_000,
        2,
    )?;
    threads::sleep_ms(1_500);
    let rr = PolicySpec::new(PolicyKind::RoundRobin, Preset::Gentle);
    sched_control::set_policy(rr).map_err(|_| "tune: the person's switch failed")?;
    let status = finished(id)?;
    if status.reason != reason::CANCELLED || !threads::installed_policy_everywhere("round-robin") {
        return Err("tune: a policy switch did not cancel the trial and win");
    }
    sched_control::set_policy(PolicySpec::DEFAULT).map_err(|_| "tune: cannot reset")?;
    log!("tune-revert: (d) a policy switch cancelled the trial and won");

    if sched_control::runaways() != runaways {
        return Err("tune: a workload process was killed as a runaway during the trials");
    }
    log!("tune-revert: no workload process was killed as a runaway");
    Ok(())
}

/// How long one shell line may take: an observe window and the wait for the
/// model's answer, as the terminal allows, or a whole trial. On one CPU under
/// icount, mix A keeps llm's thread waiting behind woken ones, so even a replayed
/// answer took about 90 s of virtual time to come back.
const SHELL_LINE_MS: u64 = llmproto::tune::CALL_TIMEOUT_MS + 10_000;

pub(crate) fn line(text: &str) -> Result<alloc::string::String, &'static str> {
    let reply = crate::ipc::call("shell", text.as_bytes(), SHELL_LINE_MS).map_err(|error| {
        log!("tune: no reply to {text:?}: {error:?}");
        "tune: the shell did not answer"
    })?;
    let reply = alloc::string::String::from_utf8_lossy(&reply).into_owned();
    log!("tune: {text:?} -> {:?}", reply.lines().next().unwrap_or(""));
    Ok(reply)
}

/// The proposal id of a `confirm <id> ...` reply.
fn proposal(reply: &str, plan: &str) -> Result<u64, &'static str> {
    if !reply.contains(plan) {
        log!("tune: expected {plan:?} in {reply:?}");
        return Err("tune: the proposal card is not the expected one");
    }
    reply
        .strip_prefix("confirm ")
        .and_then(|rest| rest.split(' ').next())
        .and_then(|id| id.parse().ok())
        .ok_or("tune: the shell did not ask for a yes")
}

static ANSWER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn answer_in_background(_: usize) {
    let id = ANSWER.load(Ordering::SeqCst);
    let request = alloc::format!("answer {id} yes");
    let _ = crate::ipc::call("shell", request.as_bytes(), SHELL_LINE_MS);
}

/// M10a-4 (`test=tune workload=A`): the shell's `tune` with the mock server's
/// canned answers, keyed by the founder's intent: a good pick is tried and its
/// verdict is the judge's; round-robin is rolled back; an invalid answer keeps
/// the policy and starts nothing; a shell killed mid-trial leaves the last good
/// policy on every CPU.
pub fn shell_check() -> Result<(), &'static str> {
    crate::shell_tests::start()?;
    pid_named("editor")?;
    threads::sleep_ms(1_000);

    // (a)
    let id = proposal(&line("tune editor: good pick")?, "Plan: try classes/firm")?;
    let verdict = line(&alloc::format!("answer {id} yes"))?;
    let kept = verdict.starts_with("ok Kept classes/firm");
    if !kept && !verdict.starts_with("ok Rolled back classes/firm") {
        return Err("tune: the good pick got no verdict card");
    }
    if sched::judge::judge(&sched_control::last_pairs(), 0).kept != kept {
        return Err("tune: the verdict card is not the judge's verdict");
    }
    if kept && line("tune undo")? != "ok back to interactive" {
        return Err("tune: undo did not put interactive back");
    }
    log!("tune: (a) the good pick's verdict is the judge's");

    // (b)
    let id = proposal(&line("tune editor: round robin")?, "Plan: try round-robin")?;
    if !line(&alloc::format!("answer {id} yes"))?.starts_with("ok Rolled back round-robin") {
        return Err("tune: round-robin was not rolled back");
    }
    log!("tune: (b) round-robin was rolled back");

    // (c)
    let before = sched_control::status().id;
    let reply = line("tune editor: invalid answer")?;
    if !reply.starts_with("ok ")
        || !reply.contains("Kept the current policy: the answer was not the JSON object asked for")
        || sched_control::status().id != before
        || !threads::installed_policy_everywhere("interactive")
    {
        return Err("tune: an invalid answer was not inert");
    }
    log!("tune: (c) an invalid answer kept the policy and started nothing");

    // (d)
    let shell = crate::shell_tests::start()?;
    let id = proposal(&line("tune editor: slow pick")?, "Plan: try classes/gentle")?;
    ANSWER.store(id, Ordering::SeqCst);
    threads::spawn("tune-answer", answer_in_background, 0)?;
    let running = (0..200).any(|_| {
        threads::sleep_ms(50);
        let status = sched_control::status();
        status.id > before && status.state == state::RUNNING
    });
    if !running {
        return Err("tune: the trial never started");
    }
    threads::sleep_ms(1_500);
    process::kill(shell, kabi::ExitReason::Killed).map_err(|_| "tune: cannot kill the shell")?;
    let status = finished(sched_control::status().id)?;
    if status.reason != reason::STARTER_EXITED
        || !threads::installed_policy_everywhere("interactive")
    {
        return Err("tune: a shell killed mid-trial did not leave the last good policy");
    }
    log!("tune: (d) the shell was killed mid-trial; every CPU is back on interactive");
    for _ in 0..200 {
        if crate::shell_tests::start().is_ok_and(|pid| pid != shell) {
            break;
        }
        threads::sleep_ms(50);
    }
    if line("1 + 1")? != "ok 2" {
        return Err("tune: the restarted shell does not answer");
    }
    Ok(())
}
