//! Plan lines the kernel gives the supervisor, kept here so the supervisor's
//! host tests parse exactly what the kernel sends.

use alloc::format;
use alloc::string::String;

/// The Rhai shell: it serves the terminal, calls the llm service through its one
/// route, may switch the scheduling policy, and needs a large stack for Rhai's
/// recursion.
pub const SHELL: &str = "shell caps=serve+call+policy+stop+script uses=llm+supervisor pages=4096 stack_pages=512 burst_ms=5000";
/// The file system service (ADR 0004): the only holder of the `kai-data`
/// partition. Planned before the shell, and only when the disk has `kai-data`.
/// Its pages hold a 16 MiB block cache and kaifs's working state.
pub const FS: &str = "fs caps=serve+partition pages=8192 burst_ms=5000";
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
