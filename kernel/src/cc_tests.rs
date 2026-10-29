//! P4 task 12's exit case (`test=c-prompt`): the `c>` prompt through the
//! ring-3 shell and the `cc` service, called exactly as the terminal calls the
//! shell. A definition, a call, a redefinition that switches its caller,
//! `:dis`, an entry left open, then a `kai` call that goes through the one
//! door, and its `undo`. Then S12-1's compile time over a 200-line prelude,
//! reported only.

use crate::sched_control::calls::MIN_POLICY_INTERVAL_NS;
use crate::shell_tests::Expect::{self, Contains, Exact, StartsWith};
use crate::shell_tests::{expect_lines, start};
use crate::{hpet, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const MODES_AND_DEFINITIONS: &[(&str, Expect)] = &[
    (":c", StartsWith("ok C mode")),
    ("I64 Sq(I64 x) { return x * x; }", Exact("ok ")),
    ("I64 Twice(I64 x) { return Sq(x) * 2; }", Exact("ok ")),
    ("Twice(3)", Exact("ok = 18")),
    // The redefinition writes Sq's table slot: Twice calls the new body.
    ("I64 Sq(I64 x) { return x + 1; }", Exact("ok ")),
    ("Twice(3)", Exact("ok = 8")),
    (":dis Twice", Contains("call Sq (slot ")),
    ("I64 F(I64 x) {", Exact("more")),
    ("  return x * 10; }", Exact("ok ")),
    ("Print(\"F(4) is %lld\\n\", F(4));", Exact("ok F(4) is 40")),
    ("char now[16];", Exact("ok ")),
    // A kai import: set_policy goes through agentcore::execute, at its tier,
    // into the action log.
    ("set_policy(\"round-robin\")", Exact("ok = 0")),
    (
        "get_policy(now, 16); (const char *)now",
        Exact("ok = \"round-robin\""),
    ),
];

/// After the policy switch's rate limit: undo skips the read and puts the
/// policy back, through the log.
const UNDO: &[(&str, Expect)] = &[
    ("undo", Contains("policy back to interactive")),
    (
        "get_policy(now, 16); (const char *)now",
        Exact("ok = \"interactive\""),
    ),
    (":rhai", Exact("ok back at kai>")),
    ("policy", Exact("ok scheduling policy: interactive")),
];

/// Definitions in an entry, five entries of them: a 200-line prelude.
const PRELUDE_ENTRIES: u32 = 5;
const PER_ENTRY: u32 = 40;
/// Entries timed over it.
const TIMED: usize = 5;
const LINE_TIMEOUT_MS: u64 = 30_000;

pub fn check() -> Result<(), &'static str> {
    let shell = start()?;
    expect_lines(MODES_AND_DEFINITIONS)?;
    threads::sleep_ms(MIN_POLICY_INTERVAL_NS / 1_000_000 + 10);
    expect_lines(UNDO)?;
    log!(
        "c-prompt: :c, definitions, a redefinition that switched its caller, :dis, a kai call and its undo, :rhai"
    );
    prelude_timing()?;
    if ipc::server_of("shell") != Some(shell) {
        return Err("c-prompt: a different shell answered by the end");
    }
    Ok(())
}

/// S12-1's bar, measured in the OS: an entry's round trip, shell to cc and
/// back, once the session holds 200 definitions. Reported, not gated: this is
/// a dev build, where wasmi runs about 5 times slower than in release.
fn prelude_timing() -> Result<(), &'static str> {
    expect_lines(&[(":c", StartsWith("ok "))])?;
    for entry in 0..PRELUDE_ENTRIES {
        let defs: Vec<String> = (0..PER_ENTRY)
            .map(|i| format!("I64 P{entry}_{i}(I64 x) {{ return x + {i}; }}"))
            .collect();
        let line = defs.join("\n");
        let reply = ipc::call("shell", line.as_bytes(), LINE_TIMEOUT_MS)
            .map_err(|_| "c-prompt: a prelude entry got no reply")?;
        if reply != b"ok " {
            log!(
                "c-prompt: prelude entry {entry}: {:?}",
                String::from_utf8_lossy(&reply)
            );
            return Err("c-prompt: a prelude entry failed");
        }
    }
    let mut us: Vec<u64> = Vec::with_capacity(TIMED);
    for _ in 0..TIMED {
        let start = hpet::nanos();
        let reply = ipc::call("shell", b"P4_39(1)", LINE_TIMEOUT_MS)
            .map_err(|_| "c-prompt: a timed entry got no reply")?;
        us.push((hpet::nanos() - start) / 1000);
        if reply != b"ok = 40" {
            log!(
                "c-prompt: timed entry: {:?}",
                String::from_utf8_lossy(&reply)
            );
            return Err("c-prompt: a timed entry got the wrong answer");
        }
    }
    us.sort_unstable();
    log!(
        "c-prompt: 200-line prelude, entry round trip p50 {} ms, max {} ms (S12-1: compile p50 at most 150 ms, release)",
        us[TIMED / 2] / 1000,
        us[TIMED - 1] / 1000
    );
    expect_lines(&[(":rhai", Exact("ok back at kai>"))])
}
