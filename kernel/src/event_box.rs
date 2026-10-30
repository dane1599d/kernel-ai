//! The box standing instructions' events wait in for the shell (P3 task 11):
//! `desktop` puts window and idle events in with EVENT_POST, and the shell takes
//! them all with EVENT_READ. The time of day is the kernel's own: each read
//! first adds a `time <minute>` for every minute of the day (UTC, from
//! [`crate::clock`]) that began since the last read, so a rule for 09:05 costs
//! no timer and no process wakes for it. The box is bounded at [`BOX`] events
//! with no allocation: past that the oldest goes and is counted, so a shell that
//! stops reading never grows kernel memory, and the next read says how many
//! were lost.
//!
//! The ring and its wake are `kabi::event_ring`, tested on the host; here is its
//! lock, the clock, and the wake of the reader's `svc_recv` (P4). A `work` post
//! only wakes the reader and takes no slot.

use core::sync::atomic::{AtomicU64, Ordering};
use kabi::event::{BOX, DROPPED, Event};
use kabi::event_ring::Ring;
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

static RING: Mutex<Ring> = Mutex::new(Ring::new());
/// A copy of the box's reader, set only under the lock with it: `svc_recv`
/// of any other server checks it first and never takes the lock.
static READER: AtomicU64 = AtomicU64::new(0);

fn with<R>(f: impl FnOnce(&mut Ring) -> R) -> R {
    without_interrupts(|| {
        let mut ring = RING.lock();
        let out = f(&mut ring);
        READER.store(ring.reader(), Ordering::SeqCst);
        out
    })
}

/// Keeps `event` (a `work` event only wakes: it takes no slot), dropping the
/// oldest when the box is full, and wakes the box's reader if it waits in
/// `svc_recv`. The wake is set under the box's lock; the reader is woken after
/// it, outside.
pub fn post(event: Event) {
    let reader = with(|ring| ring.post(event));
    if reader != 0 {
        crate::ipc::wake_server(reader);
    }
}

/// Whether process `pid` is the reader and a post came since it last looked.
pub fn woken(pid: u64) -> bool {
    pid != 0 && READER.load(Ordering::SeqCst) == pid && with(|ring| ring.woken(pid))
}

/// Takes the wake for `pid`, once: whether there was one. A wake that lands
/// as the wait's deadline passes leaves that `svc_recv` `TimedOut`, and the
/// next returns `Woken` at once.
pub fn take_wake(pid: u64) -> bool {
    pid != 0 && READER.load(Ordering::SeqCst) == pid && with(|ring| ring.take_wake(pid))
}

/// The box's reader, 0 for none (for `event-wake`).
pub fn reader() -> u64 {
    with(|ring| ring.reader())
}

/// Process `pid` ended: it reads the box no more, so a later process with its
/// pid, or none, inherits nothing.
pub fn forget(pid: u64) {
    with(|ring| ring.forget(pid));
}

fn minute_now() -> u64 {
    crate::clock::unix_seconds().map_or(0, |seconds| seconds / 60)
}

/// Takes every event for reader `pid`, which EVENT_READ admitted, oldest first,
/// after the count of those dropped (0 when none were); `pid` is the box's
/// reader from now on, whom a post wakes. The minutes begun since the last read
/// come last.
pub fn take_for(pid: u64) -> (u64, [Option<Event>; BOX]) {
    let minute = minute_now();
    with(|ring| ring.take_for(pid, minute))
}

/// EVENT_READ's text for what [`take`] gave: `dropped <n>` first when some
/// were, then one line each.
pub fn text(dropped: u64, events: &[Option<Event>]) -> alloc::string::String {
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    if dropped > 0 {
        let _ = writeln!(out, "{DROPPED}{dropped}");
    }
    for event in events.iter().flatten() {
        let _ = event.text(&mut out);
        out.push('\n');
    }
    out
}
