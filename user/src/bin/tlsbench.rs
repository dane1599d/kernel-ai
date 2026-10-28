//! Spike P5-S1 `tls-nostd`'s ring-3 figures (docs/plans/p5-networking.md §5), in
//! a hard-float program as `pkg`, `updated` and `fetch` will be (plan §4, item
//! 3): a full TLS 1.3 handshake against an in-memory test server over an ECDSA
//! and an RSA chain, records through rustls on each suite, and the bare AEADs.
//! The test server runs in the same process: ring 3 has no TCP stream until P5
//! task 2. Figures are reported against the spike's bars, not asserted; it
//! exits 0 when every handshake and transfer gives the right answer. Not for
//! merge. Boot case `test=tls-bench`.

#![no_std]
#![no_main]

extern crate alloc;

use user::{println, sys};

user::entry!(main);

fn main() -> i32 {
    let right = spike_tls::report::run("tls-bench", sys::time_ns, &mut |line| println!("{}", line));
    println!("tls-bench: done");
    if right { 0 } else { 1 }
}
