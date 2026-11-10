//! Where an online update's time goes (P5 task 10, for T's keep-alive question):
//! each chunk is a `Range` GET that GitHub redirects, so it costs two TLS
//! connections, and then its hash, its slot write and the read-back. The clock is
//! read around each phase and the times are summed over the stage:
//!
//! - **redirect**: the releases request, from the call's start to the redirect
//!   being taken: its connect, its handshake and the 302;
//! - **assets-connect**: the assets host's connect and handshake;
//! - **body**: from the assets connection being up to the chunk being whole: the
//!   request, the server, the 2 MiB;
//! - **hash**: from the chunk being whole to its write beginning, which is
//!   `kainet`'s check against the signed list;
//! - **write**, **readback**: the slot's SLOT_WRITE and SLOT_READ;
//! - **flush** and **mark**: the SLOT_FLUSH and the mark's file in
//!   `/system/updates`, which each chunk costs too.
//!
//! What no phase claims is **rest**: the checks after the last chunk (the read-back
//! of the whole image, counted under readback, and the hash of it), the metadata
//! asked again, a kept chunk's check, the running hash. A phase the stage does not
//! reach (a kept chunk has no redirect) counts nothing.

use core::cell::Cell;
use kainet::http::{Connect, HttpError, Url};
use user::sys;

/// The sums, in nanoseconds, and the call in progress. Everything is a `Cell`,
/// so the slot, the mark and the connection can all hold a shared reference.
#[derive(Default)]
pub struct Split {
    redirect: Cell<u64>,
    assets: Cell<u64>,
    body: Cell<u64>,
    hash: Cell<u64>,
    write: Cell<u64>,
    readback: Cell<u64>,
    flush: Cell<u64>,
    mark: Cell<u64>,
    /// TLS connections made by the stage's chunk requests.
    connections: Cell<u32>,
    in_call: Cell<bool>,
    call_start: Cell<u64>,
    /// The call's connects so far, and the start and length of the first two.
    connects: Cell<u32>,
    first: Cell<(u64, u64)>,
    second: Cell<(u64, u64)>,
    /// When the last chunk was whole, until its write begins.
    fetched_at: Cell<Option<u64>>,
}

fn add(sum: &Cell<u64>, ns: u64) {
    sum.set(sum.get().saturating_add(ns));
}

impl Split {
    /// A chunk's request begins.
    pub fn begin_call(&self) {
        self.in_call.set(true);
        self.call_start.set(sys::time_ns());
        self.connects.set(0);
    }

    /// The request ended (whole, cut or refused): the time is shared out.
    pub fn end_call(&self) {
        let end = sys::time_ns();
        self.in_call.set(false);
        match self.connects.get() {
            0 => {}
            1 => {
                // No redirect: the one connection is the assets'.
                let (start, took) = self.first.get();
                add(&self.assets, took);
                add(&self.body, end.saturating_sub(start + took));
            }
            _ => {
                let (start, took) = self.second.get();
                add(&self.redirect, start.saturating_sub(self.call_start.get()));
                add(&self.assets, took);
                add(&self.body, end.saturating_sub(start + took));
            }
        }
        self.fetched_at.set(Some(end));
    }

    /// A connection was made, from `start` for `took` ns, in the request in progress.
    fn connected(&self, start: u64, took: u64) {
        if !self.in_call.get() {
            return;
        }
        self.connections.set(self.connections.get() + 1);
        match self.connects.get() {
            0 => self.first.set((start, took)),
            1 => self.second.set((start, took)),
            _ => {}
        }
        self.connects.set(self.connects.get() + 1);
    }

    /// A slot write from `start` for `took` ns: it ends the hash phase.
    pub fn wrote(&self, start: u64, took: u64) {
        if let Some(fetched) = self.fetched_at.take() {
            add(&self.hash, start.saturating_sub(fetched));
        }
        add(&self.write, took);
    }

    pub fn read(&self, took: u64) {
        add(&self.readback, took);
    }

    pub fn flushed(&self, took: u64) {
        add(&self.flush, took);
    }

    pub fn marked(&self, took: u64) {
        add(&self.mark, took);
    }

    /// The line for the log, milliseconds to one place. `total_ns` is the whole
    /// stage; `rest` is what no phase claims (the checks after the last chunk, the
    /// metadata asked again, the kept chunks' read-back, the running hash).
    pub fn line(&self, total_ns: u64) -> alloc::string::String {
        let ms = |ns: u64| {
            let tenths = ns / 100_000;
            alloc::format!("{}.{}", tenths / 10, tenths % 10)
        };
        let phases = [
            self.redirect.get(),
            self.assets.get(),
            self.body.get(),
            self.hash.get(),
            self.write.get(),
            self.readback.get(),
            self.flush.get(),
            self.mark.get(),
        ];
        let rest = total_ns.saturating_sub(phases.iter().fold(0u64, |a, p| a.saturating_add(*p)));
        alloc::format!(
            "updated: split ms: redirect {}, assets-connect {}, body {}, hash {}, write {}, readback {}, flush {}, mark {}, rest {} of {} ({} TLS connections for the chunks)",
            ms(phases[0]),
            ms(phases[1]),
            ms(phases[2]),
            ms(phases[3]),
            ms(phases[4]),
            ms(phases[5]),
            ms(phases[6]),
            ms(phases[7]),
            ms(rest),
            ms(total_ns),
            self.connections.get()
        )
    }
}

/// `C`, with the time of each connection (its connect and handshake) noted.
pub struct Timed<'a, C> {
    pub inner: C,
    pub split: &'a Split,
}

impl<C: Connect> Connect for Timed<'_, C> {
    type Stream = C::Stream;

    fn connect(&mut self, url: &Url) -> Result<Self::Stream, HttpError> {
        let start = sys::time_ns();
        let stream = self.inner.connect(url);
        self.split
            .connected(start, sys::time_ns().saturating_sub(start));
        stream
    }
}
