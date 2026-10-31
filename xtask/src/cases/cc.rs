//! T, the terminal compiler (ADR 0012): the `c>` prompt through the shell and
//! the `cc` service, and `cc build`.

use super::{TestCase, case};
use crate::blklog::Data;
use std::time::Duration;

/// `fs`'s disk, for `/home/user` and `pkg`'s store.
const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

pub(super) const CASES: &[TestCase] = &[
    // P4 task 12's exit case: cc not running until the first entry starts it
    // (`start=demand`), then `:c`, definitions, a call, a redefinition that
    // switches its caller, `:dis`, an entry left open, a `kai` call through the
    // one door and its `undo`, then `:rhai`. Then S12-1's compile time,
    // reported, over a 200-line prelude.
    TestCase {
        expect: &[
            "c-prompt: cc was not running until the first entry started it",
            "c-prompt: :c, definitions, a redefinition that switched its caller, :dis, a kai call and its undo, :rhai",
        ],
        ..case("c-prompt", "test=c-prompt")
    },
    // P4 task 13's exit case: `cc build hello` builds a C app from
    // `/home/user/c/hello/`, `pkg` publishes it into `local` signed by the
    // device key it makes, installs it as generation 2 and starts it, and its
    // `init` logs. `undo` switches back, and the next build is 0.0.2. Two
    // builds take minutes when QEMU emulates the CPU.
    TestCase {
        expect: &[
            "pkg: started from generation 1: notes 1.0.0 as p-",
            "pkg: started from local: hello 0.0.1 as p-",
            "hello from C, built in the OS",
            "c-build: cc build hello built 0.0.1, and pkg installed it from local as generation 2",
            "pkg: switched back from generation 2 to 1's packages as generation 3",
            "c-build: undo switched back to generation 1, and the next build is 0.0.2",
        ],
        data: Some(VIRTIO),
        min_timeout: Some(Duration::from_secs(600)),
        ..case("c-build", "test=c-build")
    },
];
