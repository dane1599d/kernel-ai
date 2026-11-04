//! P5's TLS 1.3 client, and later its HTTP/1.1 client and verified downloads
//! (plan p5-networking, tasks 4-6), with no std: every parser of a server's
//! bytes runs in ring 3, in the program that asked for them.

#![no_std]

extern crate alloc;
#[cfg(feature = "testca")]
extern crate std;

#[cfg(feature = "server")]
pub mod bench;
pub mod chunks;
pub mod client;
pub mod download;
pub mod http;
pub mod provider;
#[cfg(feature = "testca")]
pub mod testca;
pub mod tls;
