//! The event box's ring and its wake, without locks or system calls, so the
//! host tests it (the kernel's `event_box` holds one behind a lock). Bounded at
//! [`BOX`] events with no allocation: past that the oldest goes and is
//! counted. A `work` post takes no slot: it only sets the wake, so a process
//! that posts in a loop cannot push out the window, idle and time events the
//! standing rules act on.

use crate::event::{BOX, DAY_MINUTES, Event, Kind};

pub struct Ring {
    events: [Option<Event>; BOX],
    /// Where the oldest is.
    head: usize,
    len: usize,
    /// Lost since the last read.
    dropped: u64,
    /// The minute (since 1970) of the last read, 0 before the first.
    minute: u64,
    /// The pid that reads the box, 0 before any read or once it exited: a
    /// post wakes it.
    reader: u64,
    /// A post came since the reader last read or was woken: its `svc_recv`
    /// returns `Woken` once, so a reader that never reads does not spin.
    woken: bool,
}

impl Default for Ring {
    fn default() -> Self {
        Self::new()
    }
}

impl Ring {
    pub const fn new() -> Self {
        Self {
            events: [None; BOX],
            head: 0,
            len: 0,
            dropped: 0,
            minute: 0,
            reader: 0,
            woken: false,
        }
    }

    /// Posts `event` and sets the wake: the reader to wake, 0 for none. A
    /// `work` event is the wake alone and is not kept.
    pub fn post(&mut self, event: Event) -> u64 {
        if event.kind != Kind::Work {
            self.push(event);
        }
        self.woken = true;
        self.reader
    }

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

    /// The box's reader, 0 for none.
    pub fn reader(&self) -> u64 {
        self.reader
    }

    /// Whether `pid` is the reader and a post came since it last looked.
    pub fn woken(&self, pid: u64) -> bool {
        pid != 0 && self.reader == pid && self.woken
    }

    /// Takes the wake for `pid`, once: whether there was one.
    pub fn take_wake(&mut self, pid: u64) -> bool {
        let mine = self.woken(pid);
        if mine {
            self.woken = false;
        }
        mine
    }

    /// `pid` ended: it reads the box no more, so a later process with its pid
    /// inherits nothing.
    pub fn forget(&mut self, pid: u64) {
        if self.reader == pid {
            self.reader = 0;
            self.woken = false;
        }
    }

    /// Takes every event for reader `pid`, oldest first, after the count of
    /// those dropped; `pid` is the reader from now on. `minute` is now (since
    /// 1970, 0 when unknown): the minutes of the day begun since the last read
    /// come last.
    pub fn take_for(&mut self, pid: u64, minute: u64) -> (u64, [Option<Event>; BOX]) {
        self.reader = pid;
        self.woken = false;
        self.take(minute)
    }

    /// [`Ring::take_for`] by no reader.
    pub fn take(&mut self, minute: u64) -> (u64, [Option<Event>; BOX]) {
        if minute != 0 {
            self.add_minutes(minute);
        }
        let mut out = [None; BOX];
        for (n, slot) in out.iter_mut().take(self.len).enumerate() {
            *slot = self.events[(self.head + n) % BOX].take();
        }
        let dropped = core::mem::take(&mut self.dropped);
        self.head = 0;
        self.len = 0;
        (dropped, out)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event(line: &str) -> Event {
        Event::parse(line).unwrap()
    }

    #[test]
    fn a_flood_of_work_keeps_every_window_event() {
        let mut ring = Ring::new();
        ring.post(event("close notes"));
        for _ in 0..BOX * 4 {
            ring.post(event("work"));
        }
        let (dropped, events) = ring.take_for(9, 0);
        assert_eq!(dropped, 0);
        let kept: alloc::vec::Vec<_> = events.iter().flatten().collect();
        assert_eq!(kept, [&event("close notes")]);
    }

    #[test]
    fn a_post_wakes_the_reader_once_and_a_read_clears_it() {
        let mut ring = Ring::new();
        assert_eq!(ring.post(event("work")), 0, "no reader yet");
        ring.take_for(9, 0);
        assert!(!ring.woken(9));
        assert_eq!(ring.post(event("focus notes")), 9);
        assert!(ring.woken(9) && !ring.woken(8));
        assert!(ring.take_wake(9));
        assert!(!ring.take_wake(9), "one wake a post");
        ring.post(event("work"));
        ring.take_for(9, 0);
        assert!(!ring.woken(9), "reading clears the wake");
    }

    #[test]
    fn an_ended_reader_leaves_nothing_behind() {
        let mut ring = Ring::new();
        ring.take_for(9, 0);
        ring.post(event("work"));
        ring.forget(9);
        assert_eq!(ring.reader(), 0);
        assert!(!ring.woken(9));
        ring.forget(7);
        assert_eq!(ring.post(event("work")), 0);
    }

    #[test]
    fn the_box_drops_its_oldest_and_counts_them() {
        let mut ring = Ring::new();
        for _ in 0..=BOX {
            ring.post(event("close notes"));
        }
        let (dropped, events) = ring.take(0);
        assert_eq!((dropped, events.iter().flatten().count()), (1, BOX));
    }
}
