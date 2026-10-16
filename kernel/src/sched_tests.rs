//! M4 self-tests: threads sleep and wake, a woken interactive thread beats eight CPU
//! hogs to the CPU, and a broken policy is ejected by the watchdog.

use crate::keyboard::{self, Decoder};
use crate::{log, threads};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use sched::{EnqueueReason, RoundRobin, SchedPolicy, Tid};

const HOGS: usize = 8;
const LATENCY_LIMIT_NS: u64 = 20_000_000;

static HOG_WORK: [AtomicU64; HOGS] = [const { AtomicU64::new(0) }; HOGS];
static SLEEPERS_DONE: AtomicUsize = AtomicUsize::new(0);
static TYPED_LINE_DONE: AtomicBool = AtomicBool::new(false);

fn hog(index: usize) {
    loop {
        HOG_WORK[index].fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

fn sleeper(_: usize) {
    for _ in 0..5 {
        threads::sleep_ms(10);
    }
    SLEEPERS_DONE.fetch_add(1, Ordering::SeqCst);
}

/// Waits on the boot thread, sleeping between checks, until `done()` or timeout.
fn wait_for(done: impl Fn() -> bool, timeout_ms: u64) -> bool {
    for _ in 0..timeout_ms / 5 {
        if done() {
            return true;
        }
        threads::sleep_ms(5);
    }
    done()
}

/// Boot check: two threads sleep five times each and finish.
pub fn boot_check() -> Result<(), &'static str> {
    threads::spawn("sleeper-a", sleeper, 0)?;
    threads::spawn("sleeper-b", sleeper, 1)?;
    if !wait_for(|| SLEEPERS_DONE.load(Ordering::SeqCst) == 2, 2_000) {
        return Err("threads: sleeping threads did not finish");
    }
    let (policy, _, switches) = threads::stats();
    log!("self-test: two threads slept and finished ({switches} switches, policy {policy})");
    Ok(())
}

fn spawn_hogs(count: usize) -> Result<[Tid; HOGS], &'static str> {
    let mut tids = [0; HOGS];
    for (i, tid) in tids.iter_mut().enumerate().take(count) {
        *tid = threads::spawn("hog", hog, i)?;
    }
    Ok(tids)
}

/// The interactive thread: blocks on the keyboard and records its wake latency.
fn typist(_: usize) {
    let mut decoder = Decoder::new();
    let mut line = [0u8; 8];
    let mut len = 0;
    loop {
        let ch = decoder.read_char();
        crate::serial::_print(format_args!("{ch}"));
        if matches!(ch, '\n' | '\r') {
            if &line[..len] == b"kai" {
                TYPED_LINE_DONE.store(true, Ordering::SeqCst);
                return;
            }
            len = 0;
        } else if len < line.len() && ch.is_ascii() {
            line[len] = ch as u8;
            len += 1;
        }
    }
}

/// M4 exit test: with eight CPU hogs running, typed keys reach the interactive
/// thread within 20 ms, and every hog still makes progress.
pub fn latency_check() -> Result<(), &'static str> {
    spawn_hogs(HOGS)?;
    threads::spawn("typist", typist, 0)?;
    threads::sleep_ms(100);
    log!("sched: {HOGS} hogs running; ready for keys");
    if !wait_for(|| TYPED_LINE_DONE.load(Ordering::SeqCst), 30_000) {
        return Err("sched: the typed line never reached the interactive thread");
    }
    let (wakes, worst) = keyboard::wake_latency();
    let work: [u64; HOGS] = core::array::from_fn(|i| HOG_WORK[i].load(Ordering::Relaxed));
    let (policy, ejections, switches) = threads::stats();
    log!(
        "sched: {wakes} keyboard wake-ups, worst latency {} us (limit 20000)",
        worst / 1000
    );
    log!(
        "sched: hog progress {work:?}; {switches} switches; policy {policy}, {ejections} ejections"
    );
    if worst >= LATENCY_LIMIT_NS {
        return Err("sched: keyboard wake latency was 20 ms or more");
    }
    if work.contains(&0) {
        return Err("sched: a hog never ran");
    }
    Ok(())
}

/// The thread a broken test policy refuses to schedule.
static VICTIM: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Round-robin that silently drops the victim thread: the watchdog must catch it.
struct Starver(RoundRobin);

impl SchedPolicy for Starver {
    fn name(&self) -> &'static str {
        "starver (deliberately broken)"
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
    fn reset(&mut self) {
        self.0.reset();
    }
}

/// M4 exit test: a policy that starves a thread is ejected, and the system recovers.
pub fn watchdog_check() -> Result<(), &'static str> {
    let tids = spawn_hogs(3)?;
    VICTIM.store(tids[1], Ordering::SeqCst);
    threads::set_policy(alloc::boxed::Box::new(Starver(RoundRobin::new())));
    let victim_before = HOG_WORK[1].load(Ordering::Relaxed);
    log!("sched: installed a policy that starves thread {}", tids[1]);
    threads::sleep_ms(1_000);
    let (policy, ejections, _) = threads::stats();
    let victim_work = HOG_WORK[1].load(Ordering::Relaxed) - victim_before;
    log!(
        "sched: after 1 s: policy {policy}, {ejections} ejection(s), victim progress {victim_work}"
    );
    if ejections == 0 {
        return Err("sched: the watchdog did not eject the starving policy");
    }
    if victim_work == 0 {
        return Err("sched: the starved thread never ran after the ejection");
    }
    Ok(())
}
