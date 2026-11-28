//! USB hot-plug (P7 row 5): the `usb-ports` thread services the root ports
//! that changed. `poll`, in the interrupt or the tick, only sees that a
//! controller has a port to service and wakes the thread. The thread takes
//! one step at a time (`Controller::service_step`) holding the controllers'
//! lock, and sleeps with it released for the debounce and the port reset in
//! between, so the interrupt keeps reading other devices' reports meanwhile.
//! After each step it polls, which hands on any report a step kept and
//! acknowledges an interrupt that came while the lock was held.
//!
//! The thread is started only when a controller is running, and pinned to the
//! boot CPU, where the controllers' interrupts and the tick's polls are taken.

use super::{CONTROLLERS, KEYBOARDS, KEYS, log_report, poll};
use crate::{hpet, keyboard, log, pointer, threads};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use x86_64::instructions::interrupts;
use xhci::Change;

/// The thread's tid, once it is started.
static TID: AtomicUsize = AtomicUsize::new(usize::MAX);
/// A controller has a root port to service. Set before the thread is woken,
/// and cleared by the thread before it services.
static CHANGED: AtomicBool = AtomicBool::new(false);
/// What was set up or detached since boot, for the self-test.
static ATTACHED: AtomicU64 = AtomicU64::new(0);
static DETACHED: AtomicU64 = AtomicU64::new(0);

pub(super) fn start() {
    match threads::spawn_pinned(0, "usb-ports", run, 0) {
        Ok(tid) => {
            TID.store(tid, Ordering::Release);
            log!("usb: hot-plug thread started");
        }
        Err(why) => log!("usb: no hot-plug thread ({why}); plugged devices are not set up"),
    }
}

/// From `poll`, interrupts off: a controller marked a root port as changed.
/// The thread is woken once per change it has not taken yet.
pub(super) fn changed() {
    if CHANGED.swap(true, Ordering::AcqRel) {
        return;
    }
    let tid = TID.load(Ordering::Acquire);
    if tid != usize::MAX {
        threads::wake(tid);
    }
}

fn run(_: usize) {
    loop {
        // A plain atomic load, as `block_unless` requires; `changed` sets the
        // flag before it wakes, so no change is missed.
        threads::block_unless(|| CHANGED.load(Ordering::Acquire));
        CHANGED.store(false, Ordering::Release);
        while let Some(wait) = step() {
            deliver();
            match wait {
                0 => threads::yield_now(),
                ns => threads::sleep_ms(ns.div_ceil(1_000_000)),
            }
        }
        deliver();
    }
}

/// One step on each controller, holding the lock only for it; the shortest
/// wait any of them asks for, or `None` when none has more to do.
fn step() -> Option<u64> {
    let mut controllers = CONTROLLERS.lock();
    let mut wait: Option<u64> = None;
    for (controller, _) in controllers.iter_mut() {
        if let Some(ns) = controller.service_step(&mut report) {
            wait = Some(wait.map_or(ns, |w| w.min(ns)));
        }
    }
    wait
}

/// Polls the controllers as their interrupt and the tick do, and wakes the
/// reader a key went to.
fn deliver() {
    for on_irq in [true, false] {
        interrupts::without_interrupts(|| {
            if let Some(tid) = poll(on_irq) {
                threads::wake(tid);
            }
        });
    }
}

fn report(change: Change) {
    match change {
        Change::Attached(report) => {
            ATTACHED.fetch_add(1, Ordering::Relaxed);
            log_report(report);
        }
        Change::Detached { port, route: 0 } => {
            DETACHED.fetch_add(1, Ordering::Relaxed);
            log!("usb: port {port}: detached");
        }
        Change::Detached { port, route } => {
            DETACHED.fetch_add(1, Ordering::Relaxed);
            log!("usb: port {port}, route {route:#x}: detached");
        }
    }
}

/// The lines `test=usb-hotplug` types, in order: "hot" while a mouse is
/// plugged in, "one" after the mouse is unplugged, "two" on a keyboard
/// plugged in after the first is unplugged.
const LINES: [&str; 3] = ["hot", "one", "two"];

/// P7 row 5 self-test (`test=usb-hotplug`). The harness plugs in a USB mouse
/// and types "hot" on the USB keyboard while the mouse is set up, moves the
/// mouse, unplugs it and types "one", then unplugs the keyboard, plugs in
/// another and types "two" on it. Every line must come whole and over USB,
/// so no key was lost to a hot-plug.
pub fn self_test() -> Result<(), &'static str> {
    const TIMEOUT_NS: u64 = 90_000_000_000;
    if KEYBOARDS.load(Ordering::Relaxed) == 0 {
        return Err("usb: no USB keyboard was set up");
    }
    if TID.load(Ordering::Acquire) == usize::MAX {
        return Err("usb: no hot-plug thread");
    }
    log!("usb: ready for keys (\"hot\" while a mouse is plugged in, then \"one\" and \"two\")");
    let start = hpet::nanos();
    let mut decoder = keyboard::Decoder::new();
    let mut line = alloc::string::String::new();
    let (mut next, mut moved) = (0, false);
    let mut keys_at = KEYS.load(Ordering::Relaxed);
    while next < LINES.len() {
        while let Some(key) = decoder.next_key() {
            match key {
                termcore::keys::Key::Enter => {
                    if line != LINES[next] {
                        log!(
                            "usb: hot-plug: {line:?} came where {:?} was typed",
                            LINES[next]
                        );
                        return Err("usb: a key was lost during a hot-plug");
                    }
                    let keys = KEYS.load(Ordering::Relaxed) - keys_at;
                    // Each letter and Enter, down and up.
                    if keys < 2 * (line.len() as u64 + 1) {
                        return Err("usb: a line came from another keyboard, not over USB");
                    }
                    log!("usb: hot-plug: {line:?} typed whole over USB ({keys} key changes)");
                    keys_at = KEYS.load(Ordering::Relaxed);
                    line.clear();
                    next += 1;
                }
                termcore::keys::Key::Char(ch) => line.push(ch),
                _ => {}
            }
        }
        while let Some(event) = pointer::next_event() {
            moved |= event.source == pointer::Source::UsbMouse;
        }
        if hpet::nanos() - start > TIMEOUT_NS {
            return Err("usb: the hot-plug lines did not all arrive within 90 s");
        }
        interrupts::enable_and_hlt();
    }
    if !moved {
        return Err("usb: the plugged-in mouse never moved");
    }
    let (attached, detached) = (
        ATTACHED.load(Ordering::Relaxed),
        DETACHED.load(Ordering::Relaxed),
    );
    // The mouse and the second keyboard; the mouse and the first keyboard.
    if attached < 2 || detached < 2 {
        log!("usb: hot-plug: {attached} attached, {detached} detached");
        return Err("usb: a plugged or unplugged device was not seen");
    }
    log!("usb: hot-plug: {attached} attached and {detached} detached, and no key lost");
    Ok(())
}
