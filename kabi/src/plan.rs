//! Plan lines the kernel gives the supervisor, kept here so the supervisor's
//! host tests parse exactly what the kernel sends.

/// The Rhai shell: it serves the terminal, calls the llm service, may switch
/// the scheduling policy, and needs a large stack for Rhai's recursion.
pub const SHELL: &str = "shell caps=serve+call+policy pages=4096 stack_pages=512 burst_ms=5000";
/// The live `notes` component, run by the shared `comphost` program. Its
/// arguments (`version=` and the probation settings) go to the host. It starts on
/// version 2, built from `components/notes`, which `:edit notes` edits.
pub const NOTES: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=2";
/// For the M9a test: version 1, with probation ended by calls alone, so the test
/// runs the same under KVM and TCG.
pub const NOTES_TEST: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=1 probation_calls=3 probation_ms=0";
/// For the M9b test: version 2, with probation ended by calls alone.
pub const NOTES_EDIT_TEST: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=2 probation_calls=3 probation_ms=0";
/// The edit service, planned only when the kernel has a build service. The
/// kernel appends `host=IP:PORT token=HEX`.
pub const EDIT_BASE: &str = "edit caps=serve+publish pages=2048 burst_ms=10000";
