//! The box standing instructions' events wait in for the shell (P3 task 11):
//! `desktop` puts window and idle events in with EVENT_POST, and the shell takes
//! them all with EVENT_READ. The time of day is the kernel's own: each read
//! first adds a `time <minute>` for every minute of the day (UTC, from
//! [`crate::clock`]) that began since the last read, so a rule for 09:05 costs
//! no timer and no process wakes for it. The box is bounded at [`BOX`] events
//! with no allocation: past that the oldest goes and is counted, so a shell that
//! stops reading never grows kernel memory, and the next read says how many
//! were lost.

use kabi::event::{BOX, DAY_MINUTES, DROPPED, Event, Kind};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

struct Ring {
    events: [Option<Event>; BOX],
    /// Where the oldest is.
    head: usize,
    len: usize,
    /// Lost since the last read.
    dropped: u64,
    /// The minute (since 1970) of the last read, 0 before the first.
    minute: u64,
}

impl Ring {
    /// Keeps `event`, dropping the oldest when the box is full.
    fn push(&mut self, event: Event) {
        if self.len == BOX {
            self.head = (self.head + 1) % BOX;
            self.len -= 1;
            self.dropped += 1;
        }
        let at = (self.head + self.len) % BOX;
        self.events[at] = Some(event);
        self.len += 1;
    }

    /// Adds the minutes of the day that began since the last read: at most a
    /// box's worth, the rest counted as dropped.
    fn add_minutes(&mut self, now: u64) {
        let last = core::mem::replace(&mut self.minute, now);
        if last == 0 || now <= last {
            return;
        }
        let begun = now - last;
        let kept = begun.min(BOX as u64);
        self.dropped += begun - kept;
        for minute in now + 1 - kept..=now {
            let of_day = (minute % u64::from(DAY_MINUTES)) as u16;
            if let Some(event) = Event::timed(Kind::Time(of_day)) {
                self.push(event);
            }
        }
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring {
    events: [None; BOX],
    head: 0,
    len: 0,
    dropped: 0,
    minute: 0,
});

fn with<R>(f: impl FnOnce(&mut Ring) -> R) -> R {
    without_interrupts(|| f(&mut RING.lock()))
}

/// Keeps `event`, dropping the oldest when the box is full.
pub fn post(event: Event) {
    with(|ring| ring.push(event))
}

/// Takes every event, oldest first, after the count of those dropped (0 when
/// none were). The minutes begun since the last read come last.
pub fn take() -> (u64, [Option<Event>; BOX]) {
    let minute = crate::clock::unix_seconds().map_or(0, |seconds| seconds / 60);
    with(|ring| {
        if minute != 0 {
            ring.add_minutes(minute);
        }
        let mut out = [None; BOX];
        for (n, slot) in out.iter_mut().take(ring.len).enumerate() {
            *slot = ring.events[(ring.head + n) % BOX].take();
        }
        let dropped = core::mem::take(&mut ring.dropped);
        ring.head = 0;
        ring.len = 0;
        (dropped, out)
    })
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
