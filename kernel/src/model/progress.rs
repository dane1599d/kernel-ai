//! The progress line while the model is copied from its disk: every tenth of the
//! file, how far, how fast, and how much of the time went to the disk rather than
//! to the copy, so a long load never looks like a hang and a slow one says why.

use crate::{hpet, log};

const MIB: u64 = 1024 * 1024;

pub struct Progress {
    len: u64,
    started: u64,
    /// Nanoseconds spent copying into the runs (the rest is the disk and the
    /// driver).
    copying: u64,
    /// The tenths of the file reported so far.
    reported: u64,
}

impl Progress {
    pub fn new(len: u64) -> Self {
        Self {
            len,
            started: hpet::nanos(),
            copying: 0,
            reported: 0,
        }
    }

    /// Records a piece that took `copy_ns` to copy, with `done` bytes copied in
    /// all, and logs each tenth passed.
    pub fn piece(&mut self, done: u64, copy_ns: u64) {
        self.copying += copy_ns;
        let tenths = done * 10 / self.len.max(1);
        if tenths <= self.reported || tenths >= 10 {
            return;
        }
        self.reported = tenths;
        let elapsed = (hpet::nanos() - self.started).max(1);
        let disk = elapsed.saturating_sub(self.copying);
        log!(
            "model: {}% ({} of {} MiB), {} MB/s, {}% of the time reading the disk",
            tenths * 10,
            done / MIB,
            self.len / MIB,
            done * 1000 / elapsed,
            disk * 100 / elapsed
        );
    }
}
