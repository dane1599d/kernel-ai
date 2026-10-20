//! Runtime for ring-3 programs: the entry point, system calls, a heap, console
//! output and a panic handler that exits the process.
//!
//! A program is a `src/bin/*.rs` file with `#![no_std]`, `#![no_main]` and
//! `user::entry!(main);` where `main` is `fn() -> i32`.

#![no_std]

extern crate alloc;

pub mod heap;
pub mod pool;
pub mod sys;

use core::fmt::{self, Write};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting::new();

/// Bytes of heap this program has in use.
pub fn heap_bytes() -> u64 {
    HEAP.live_bytes()
}

/// Defines `_start`, which runs `main` and exits with its return value.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn _start() -> ! {
            $crate::sys::exit($main())
        }
    };
}

/// Console output. Each `write` system call is shown with the program's name.
pub struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        sys::write(s.as_bytes()).map(|_| ()).map_err(|_| fmt::Error)
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    // Formatted into one buffer so a line reaches the console in one call.
    let text = alloc::fmt::format(args);
    let _ = sys::write(text.as_bytes());
}

/// Prints a line to the console.
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {
        $crate::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}

/// The argument string this program was started with.
pub fn args() -> alloc::string::String {
    let mut buf = [0u8; kabi::MAX_ARGS];
    let len = sys::args(&mut buf).unwrap_or(0).min(buf.len());
    alloc::string::String::from_utf8_lossy(&buf[..len]).into_owned()
}

/// The value of `key=value` in the argument string.
pub fn arg<'a>(args: &'a str, key: &str) -> Option<&'a str> {
    args.split_whitespace()
        .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Written without the heap, which may be what failed.
    let _ = writeln!(Console, "panic: {info}");
    sys::exit(101)
}
