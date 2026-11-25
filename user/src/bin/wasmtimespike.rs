//! S14-2 (spike/wasmtime-nostd, never merged): Wasmtime's `no_std` runtime in a
//! ring-3 process. It loads `hello.cwasm`, compiled ahead of time for Pulley by
//! `spikes/s14-2-host`, and calls two exports: `fib(20)` must be 6765, and `sum`
//! over the 8 bytes of the module's data segment must be 36.

#![no_std]
#![no_main]

use user::println;

user::entry!(main);

static CWASM: &[u8] = include_bytes!("../../../spikes/s14-2/hello.cwasm");

fn main() -> i32 {
    match spike_wasmtime_nostd::run(CWASM, 20, 8) {
        Ok((6765, 36)) => {
            println!("wasmtime-nostd: fib(20) = 6765 and the data segment sums to 36");
            0
        }
        Ok((fib, sum)) => {
            println!("wasmtime-nostd: wrong answers: fib(20) = {fib}, sum = {sum}");
            1
        }
        Err(why) => {
            println!("wasmtime-nostd: failed: {why}");
            2
        }
    }
}
