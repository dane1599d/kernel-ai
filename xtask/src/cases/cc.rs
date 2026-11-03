//! T, the terminal compiler (ADR 0012): the `c>` prompt through the shell and
//! the `cc` service, and `cc build`.

use super::{TestCase, case};
use crate::blklog::Data;
use std::time::Duration;

/// fs, pkg, notes, the model, the shell, cc (8,192 pages) and the built app
/// together pass the page budget (half the free memory) of the default
/// 512 MiB guest.
const C_BUILD_MIB: u64 = 1024;

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
    // P4 task 13's exit case: generation 1 is the image's packages, which
    // start no app (#536). `cc build hello` builds a C app the image does not
    // ship from `/home/user/c/hello/`; `pkg` publishes it into `local` signed
    // by the device key it makes, installs it as generation 2 and starts it,
    // and its `init` logs; the kernel waits until the app runs 0.0.1. `undo`
    // switches back and ends it, which only a running app can be, and the
    // next build is 0.0.2, which the app's new host swaps to from the
    // withdrawn 0.0.1 (#572). Two builds take minutes when QEMU emulates the
    // CPU.
    TestCase {
        expect: &[
            "pkg: generation 1 starts no app from a package",
            "pkg: started from local: hello 0.0.1 as p-",
            "hello from C, built in the OS",
            "c-build: cc build hello built 0.0.1, and pkg installed it from local as generation 2",
            "c-build: hello 0.0.1 runs as p-",
            "c-build: cc tried hello through a read-only Shm: trial ok: init, started; 0 window(s), 0 frame(s); 0 effect(s) queued, none run",
            "c-build: a build through a read-only Shm is refused: internal: cc cannot map the Shm read-write",
            "pkg: switched back from generation 2 to 1's packages as generation 3",
            "pkg: ended, undone: hello as p-",
            "c-build: undo switched back to generation 1, and the next build is 0.0.2",
            "pkg: started from local: hello 0.0.2 as p-",
            "c-build: hello 0.0.2 runs as p-",
        ],
        data: Some(VIRTIO),
        memory_mib: Some(C_BUILD_MIB),
        min_timeout: Some(Duration::from_secs(600)),
        ..case("c-build", "test=c-build")
    },
];
