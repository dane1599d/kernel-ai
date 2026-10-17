//! Plan lines the kernel gives the supervisor, kept here so the supervisor's
//! host tests parse exactly what the kernel sends.

/// The Rhai shell: it serves the terminal, calls the llm service, may switch
/// the scheduling policy, and needs a large stack for Rhai's recursion.
pub const SHELL: &str = "shell caps=serve+call+policy pages=4096 stack_pages=512 burst_ms=5000";
/// The live `notes` component, run by the shared `comphost` program. Its
/// arguments (`version=` and the probation settings) go to the host.
pub const NOTES: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=1";
/// As [`NOTES`], with probation ended by calls alone, so the M9a test runs the
/// same under KVM and TCG.
pub const NOTES_TEST: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=1 probation_calls=3 probation_ms=0";
