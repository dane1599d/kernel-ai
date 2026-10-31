//! Plan lines the kernel gives the supervisor, kept here so the supervisor's
//! host tests parse exactly what the kernel sends.

use alloc::format;
use alloc::string::String;

/// The Rhai shell: it serves the terminal, calls the llm service through its one
/// route, may switch the scheduling policy, and needs a large stack for Rhai's
/// recursion.
pub const SHELL: &str = "shell caps=serve+call+policy+stop+script uses=llm+supervisor+cc pages=4096 stack_pages=512 burst_ms=5000";
/// [`SHELL`] once `fs` runs: with `files`, its `Dir` for `/home/user`, for the
/// file rows (P2 task 11), and `state`, its `Dir` for `/system/state`, for the
/// action log (P2 task 12).
pub const SHELL_FILES: &str = "shell caps=serve+call+policy+stop+script+files+state uses=llm+supervisor+cc pages=4096 stack_pages=512 burst_ms=5000";
/// [`SHELL`] on a desktop: with a route to `desktop`, fourth, after `cc`, which
/// it takes the AI panel's requests from and reports their states to (P3 task 7).
pub const SHELL_DESK: &str = "shell caps=serve+call+policy+stop+script uses=llm+supervisor+cc+desktop pages=4096 stack_pages=512 burst_ms=5000";
/// The `c>` prompt's compiler (ADR 0012 D2; P4 task 12): xcc's cc.wasm and the
/// sessions, on wasmi, holding no authority. Planned before the shell, which
/// calls it, and started on demand: the first `c>` entry's call starts it, so
/// it holds no memory before then. Its pages cover `ccserve::budget`: the
/// program, its stack, the compiler and one compile, one session and the
/// modules it keeps (P4 plan's 3,680 counted the heap alone).
pub const CC: &str = "cc start=demand caps=serve pages=5120 stack_pages=512 burst_ms=5000";
/// The file system service (ADR 0004): the only holder of the `kai-data`
/// partition. Planned before the shell, and only when the disk has `kai-data`.
/// Its pages hold a 16 MiB block cache and kaifs's working state. `kai-fs 1`'s
/// minicbor decoder and the request paths outgrew the default 16-page stack,
/// as filecheck's and `pkg`'s did.
pub const FS: &str = "fs caps=serve+partition pages=8192 stack_pages=64 burst_ms=5000";
/// The update service (ADR 0008 §10): the only holder of the slots and the boot
/// record. `kaituf`'s JSON and `kai-fs 1`'s CBOR decoders recurse, hence the
/// larger stack, which counts within its pages with a 512 KiB root buffer and
/// two 128 KiB slot buffers.
pub const UPDATED: &str =
    "updated caps=update dir=trust+updates+settings pages=2048 stack_pages=128 burst_ms=5000";
/// The live `notes` component, run by the shared `comphost` program, without its
/// arguments.
const NOTES_BASE: &str =
    "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000";
/// The live `notes` component starting on `version`, falling back to `fallback`
/// when that cannot start. Its arguments go to the host. Version 2 is built from
/// `components/notes`, which `:edit notes` edits; the boot manifest may name a
/// version committed in git instead (M9d).
pub fn notes(version: &str, fallback: Option<&str>) -> String {
    match fallback {
        Some(fallback) => format!("{NOTES_BASE} version={version} fallback={fallback}"),
        None => format!("{NOTES_BASE} version={version}"),
    }
}
/// Probation ended by calls alone, so a test runs the same under KVM and TCG.
pub const CALLS_ONLY: &str = "probation_calls=3 probation_ms=0";
/// For the M9a test: version 1, with probation ended by calls alone, so the test
/// runs the same under KVM and TCG.
pub const NOTES_TEST: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=1 probation_calls=3 probation_ms=0";
/// For the M9b test: version 2, with probation ended by calls alone.
pub const NOTES_EDIT_TEST: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=2 probation_calls=3 probation_ms=0";
/// The edit service, planned only when the kernel has a build service. The
/// kernel appends `host=IP:PORT token=HEX`.
pub const EDIT_BASE: &str = "edit caps=serve+publish pages=2048 burst_ms=10000";
