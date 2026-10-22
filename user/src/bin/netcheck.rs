//! Test service for RESOLVE: `host=NAME expect=A.B.C.D` resolves NAME and exits
//! 0 if the first address is the expected one; `expect=permission` exits 0 if
//! the kernel refuses the call for want of the NET capability.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use core::net::Ipv4Addr;
use kabi::Error;
use user::{arg, args, println, sys};

user::entry!(main);

fn main() -> i32 {
    let args = args();
    let (Some(host), Some(expect)) = (arg(&args, "host"), arg(&args, "expect")) else {
        println!("usage: netcheck host=NAME expect=A.B.C.D|permission");
        return 2;
    };
    let mut addresses = [[0u8; 4]; kabi::net::MAX_ADDRESSES];
    match (sys::resolve(host, &mut addresses), expect) {
        (Err(Error::Permission), "permission") => {
            println!("resolve {host} refused: no NET capability");
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
