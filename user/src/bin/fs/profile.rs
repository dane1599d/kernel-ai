//! Where a phase of client requests spends its time (#262's breakdown, for a
//! ring-3 client): with `profile=phases` on its plan line, `fs` logs one line
//! at each client Commit or Sync, for the requests since the last: the wall
//! time, the time outside `fs` (IPC and the client), and the time in `fs`,
//! split into each PART_* call's count and time and the rest (fsserve and
//! kaifs: lookups, record rewrites, checksums). Off, it costs two clock reads
//! a PART_* call.

use core::sync::atomic::{AtomicU64, Ordering};
use fsproto::Request;
use user::{println, sys};

/// Each PART_* call's count and time since the last line.
static CALLS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
pub const READ: usize = 0;
pub const WRITE: usize = 2;
pub const FLUSH: usize = 4;

/// Times `call` as PART_* call `which`.
pub fn timed<T>(which: usize, call: impl FnOnce() -> T) -> T {
    let start = sys::time_ns();
    let out = call();
    CALLS[which].fetch_add(1, Ordering::Relaxed);
    CALLS[which + 1].fetch_add(sys::time_ns().saturating_sub(start), Ordering::Relaxed);
    out
}

/// PART_READ, PART_WRITE and PART_FLUSH since the last take: (count, ns) each.
fn take() -> [(u64, u64); 3] {
    let at = |i: usize| CALLS[i].swap(0, Ordering::Relaxed);
    [READ, WRITE, FLUSH].map(|i| (at(i), at(i + 1)))
}

/// The phase so far: when its first request came, how many, and the time
/// spent in `fs` answering them.
#[derive(Default)]
pub struct Phases {
    on: bool,
    began: Option<u64>,
    requests: u64,
    inside: u64,
}

impl Phases {
    /// On when `args` has `profile=phases`.
    pub fn new(args: &str) -> Self {
        Self {
            on: args.split_whitespace().any(|w| w == "profile=phases"),
            ..Self::default()
        }
    }

    /// One client request answered between `start` and `end`; a Commit or a
    /// Sync ends the phase with its line.
    pub fn answered(&mut self, request: &[u8], start: u64, end: u64) {
        if !self.on {
            return;
        }
        let began = *self.began.get_or_insert(start);
        self.requests += 1;
        self.inside += end.saturating_sub(start);
        let ends = matches!(
            fsproto::decode_request(request),
            Ok(Request::Commit | Request::Sync)
        );
        if ends {
            self.report(end.saturating_sub(began));
        }
    }

    fn report(&mut self, wall: u64) {
        let ms = |ns: u64| ns / 1_000_000;
        let [read, write, flush] = take();
        let disk = read.1 + write.1 + flush.1;
        println!(
            "fs: phase: {} requests in {} ms; outside fs (IPC and the client) {} ms; in fs {} ms: PART_READ {}x {} ms, PART_WRITE {}x {} ms, PART_FLUSH {}x {} ms, the rest (fsserve and kaifs) {} ms",
            self.requests,
            ms(wall),
            ms(wall.saturating_sub(self.inside)),
            ms(self.inside),
            read.0,
            ms(read.1),
            write.0,
            ms(write.1),
            flush.0,
            ms(flush.1),
            ms(self.inside.saturating_sub(disk))
        );
        self.began = None;
        self.requests = 0;
        self.inside = 0;
    }
}
