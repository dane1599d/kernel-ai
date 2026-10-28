//! Spike P5-S1's size probe: only what a TLS client links, measured as its
//! `.text` against `tlsnone` (P5-S1's bar: at most 600 KiB). A client config on
//! our provider with Mozilla's roots, one connection, and its records driven
//! from bytes the optimiser cannot see, so every path stays linked. It is never
//! booted. Not for merge.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use rustls::client::UnbufferedClientConnection;
use rustls::pki_types::ServerName;
use spike_tls::pipe;
use user::{println, sys};

user::entry!(main);

static INPUT: [u8; 64] = [0x16; 64];

fn main() -> i32 {
    let time = Arc::new(spike_tls::FixedTime(sys::time_ns()));
    let Ok(config) =
        spike_tls::client_config(spike_tls::suites(), spike_tls::mozilla_roots(), time)
    else {
        return 1;
    };
    let Ok(name) = ServerName::try_from("github.com") else {
        return 1;
    };
    let Ok(mut conn) = UnbufferedClientConnection::new(config, name.to_owned()) else {
        return 1;
    };
    let mut incoming: Vec<u8> = core::hint::black_box(&INPUT).to_vec();
    let mut outgoing = Vec::new();
    let mut request: &[u8] = core::hint::black_box(b"GET / HTTP/1.1\r\n\r\n");
    let mut received = 0;
    let result = pipe::turn(
        &mut conn,
        &mut incoming,
        &mut outgoing,
        &mut request,
        usize::MAX,
        &mut |payload| received += payload.len(),
    );
    println!(
        "tlssize: {} {} {}",
        result.is_ok(),
        outgoing.len(),
        received
    );
    0
}
