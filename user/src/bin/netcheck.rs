//! Test service for resolving on a `NetDest` handle (P5 task 1), started by the
//! kernel's `net-setup` and `net-dest` cases with the handle moved in:
//! - `host=NAME expect=A.B.C.D`: RESOLVE_ON with its startup `NetDest`, and
//!   exits 0 if the first address is the expected one;
//! - `expect=permission`: exits 0 if the kernel refuses it (Permission);
//! - `narrow=SPEC`: first makes a narrower handle from it, and resolves on that;
//!   `expect=narrow-refused` exits 0 if the kernel refuses to narrow;
//! - `legacy`: uses the retired RESOLVE call instead, which always refuses.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use core::net::Ipv4Addr;
use kabi::Error;
use kabi::handle::{Handle, HandleInfo, Kind, Rights};
use user::{arg, args, println, sys};

user::entry!(main);

/// The startup `NetDest` handle, if the kernel moved one in.
fn startup_dest() -> Option<Handle> {
    let mut list = [HandleInfo::default(); 16];
    let count = sys::startup(&mut list).ok()?.min(list.len());
    list[..count]
        .iter()
        .find(|info| info.kind == Kind::NetDest as u32)
        .map(|info| Handle(info.handle))
}

fn main() -> i32 {
    let args = args();
    let (Some(host), Some(expect)) = (arg(&args, "host"), arg(&args, "expect")) else {
        println!(
            "usage: netcheck host=NAME expect=A.B.C.D|permission|narrow-refused [narrow=SPEC] [legacy]"
        );
        return 2;
    };
    let mut addresses = [[0u8; 4]; kabi::net::MAX_ADDRESSES];
    let answer = match (
        args.split_whitespace().any(|w| w == "legacy"),
        startup_dest(),
    ) {
        (true, _) => sys::resolve(host, &mut addresses),
        (false, None) => Err(Error::Permission),
        (false, Some(dest)) => {
            let dest = match arg(&args, "narrow") {
                None => dest,
                Some(spec) => {
                    let rights = (Rights::RESOLVE | Rights::INSPECT).bits();
                    match (sys::netdest_narrow(dest, spec, rights), expect) {
                        (Err(Error::Permission), "narrow-refused") => {
                            println!("narrowing to {spec} refused: it would allow more");
                            return 0;
                        }
                        (Ok(narrow), _) => narrow,
                        (Err(error), _) => {
                            println!("narrowing to {spec}: unexpected {error:?}");
                            return 1;
                        }
                    }
                }
            };
            sys::resolve_on(dest, host, &mut addresses)
        }
    };
    match (answer, expect) {
        (Err(Error::Permission), "permission") => {
            println!("resolve {host} refused: not under this handle's hosts");
            0
        }
        (Ok(count), expected) if count > 0 => {
            let got = format!("{}", Ipv4Addr::from(addresses[0]));
            println!("resolved {host} to {got} ({count} address(es))");
            i32::from(got != expected)
        }
        (other, _) => {
            println!("resolve {host}: unexpected {other:?}");
            1
        }
    }
}
