//! Test program for worker threads (L6). The kernel's thread cases start it with
//! `cap::THREADS` and a mode, and judge it by how it ends:
//!
//! - `mode=basic`: workers on every CPU, the limits and refusals of THREAD_SPAWN
//!   and THREAD_EXIT, the worker call filter, then `exit(0)` from main while
//!   workers still run. Exits 0, or the code of the first check that failed.
//! - `mode=overflow`: a worker overflows its stack and must die of a page fault in
//!   its window's guard (the kernel expects Crashed with vector 14).
//! - `mode=spawnexit`: main exits straight after starting workers (exit 0); the
//!   next process may take its slot at once.
//! - `mode=sleep`: every thread sleeps an hour at a time until the kernel kills it.
//! - `mode=nocap`: started without `cap::THREADS`; THREAD_SPAWN must say
//!   Permission (exit 0).

#![no_std]
#![no_main]

#[path = "workers/basic.rs"]
mod basic;

use user::{arg, args, println};

user::entry!(main);

fn main() -> i32 {
    let args = args();
    match arg(&args, "mode") {
        Some("basic") => basic::run(),
        Some("overflow") => basic::overflow(),
        Some("spawnexit") => basic::spawn_exit(),
        Some("sleep") => basic::sleep(),
        Some("nocap") => basic::nocap(),
        other => {
            println!("workers: unknown mode {other:?}");
            1
        }
    }
}
