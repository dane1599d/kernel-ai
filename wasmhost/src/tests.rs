//! Each kind of fault from hand-written modules, and the dev-build hazards.

extern crate std;

use super::*;
use alloc::string::String;
use alloc::vec::Vec;
use livecore::Instance;
use std::sync::Mutex;

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn sink(line: &str) {
    LOGGED.lock().unwrap().push(String::from(line));
}

/// A lifecycle module whose `kai_handle` body is `handle` (it may use $log), with
/// `extra` added to the module.
fn module(handle: &str, extra: &str) -> Vec<u8> {
    module_with("", handle, extra)
}

/// As [`module`], with `imports` after the log import.
fn module_with(imports: &str, handle: &str, extra: &str) -> Vec<u8> {
    let text = std::format!(
        r#"(module
  (import "kai" "log" (func $log (param i32 i32)))
  {imports}
  (memory (export "memory") 1)
  (global $next (mut i32) (i32.const 1024))
  (data (i32.const 16) "ok hi")
  (func (export "kai_alloc") (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $next))
    (global.set $next (i32.add (global.get $next) (local.get $len)))
    (local.get $p))
  (func (export "kai_schema") (result i32) (i32.const 1))
  (func (export "kai_init") (result i32) (i32.const 0))
  (func (export "kai_import") (param i32 i32 i32) (result i32) (i32.const 0))
  (func (export "kai_export") (param i32) (result i64) (i64.const 0x0000001000000005))
  (func (export "kai_handle") (param $p i32) (param $l i32) (result i64) {handle})
  (func (export "kai_health") (result i32) (i32.const 0))
  (func (export "kai_self_test") (result i32) (i32.const 0))
  {extra})"#
    );
    wat::parse_str(text).unwrap()
}

const REPLY: &str = "(i64.const 0x0000001000000005)";

fn load_ok(handle: &str) -> WasmInstance {
    load(&module(handle, ""), LIMITS, Some(sink)).unwrap()
}

fn reason(result: Result<impl Sized, Fault>) -> Reason {
    match result {
        Ok(_) => panic!("expected a fault"),
        Err(fault) => fault.reason,
    }
}

#[test]
fn the_lifecycle_calls_round_trip() {
    let handle = std::format!("(call $log (i32.const 16) (i32.const 5)) {REPLY}");
    let mut instance = load_ok(&handle);
    assert_eq!(instance.schema(), Ok(1));
    assert_eq!(instance.init(), Ok(0));
    assert_eq!(instance.import(b"state", 1), Ok(0));
    assert_eq!(instance.handle(b"hello").unwrap(), b"ok hi");
    assert_eq!(instance.export(0).unwrap().unwrap(), b"ok hi");
    assert_eq!(instance.health(), Ok(0));
    assert_eq!(instance.self_test(), Ok(0));
    assert!(LOGGED.lock().unwrap().iter().any(|line| line == "ok hi"));
}

#[test]
fn a_loop_that_never_ends_runs_out_of_fuel_the_same_way_every_time() {
    let spin = "(loop $l (br $l)) (unreachable)";
    for _ in 0..2 {
        let mut instance = load_ok(spin);
        let fault = instance.handle(b"x").unwrap_err();
        assert_eq!(fault.reason, Reason::Fuel, "{fault:?}");
    }
}

#[test]
fn traps_memory_limits_and_bad_results_are_told_apart() {
    assert_eq!(reason(load_ok("(unreachable)").handle(b"")), Reason::Trap);
    // 64 pages is 4 MiB, over the 2 MiB limit.
    let grow = "(drop (memory.grow (i32.const 64))) (i64.const 0)";
    assert_eq!(reason(load_ok(grow).handle(b"")), Reason::Memory);
    let outside = "(i64.const 0x7fff000000000010)";
    assert_eq!(reason(load_ok(outside).handle(b"")), Reason::Abi);
    let huge = "(i64.const 0x0000001000100000)";
    assert_eq!(reason(load_ok(huge).handle(b"")), Reason::Abi);
    let bad_log = std::format!("(call $log (i32.const -16) (i32.const 5)) {REPLY}");
    assert_eq!(reason(load_ok(&bad_log).handle(b"")), Reason::Host);
}

#[test]
fn a_module_that_cannot_run_here_is_refused_with_why() {
    let teleport = r#"(import "kai" "teleport" (func $t))"#;
    assert_eq!(
        reason(load(&module_with(teleport, REPLY, ""), LIMITS, None)),
        Reason::Link
    );
    assert_eq!(reason(load(b"not wasm", LIMITS, None)), Reason::Compile);
    let no_health = wat::parse_str(
        r#"(module (memory (export "memory") 1)
             (func (export "kai_alloc") (param i32) (result i32) (i32.const 0)))"#,
    )
    .unwrap();
    assert_eq!(reason(load(&no_health, LIMITS, None)), Reason::Abi);
    let start = "(func $s) (start $s)";
    let refused = reason(load(&module(REPLY, start), LIMITS, None));
    assert!(
        matches!(refused, Reason::Compile | Reason::Link),
        "{refused:?}"
    );
    let tail_call = "(func $t (return_call $t))";
    assert_eq!(
        reason(load(&module(REPLY, tail_call), LIMITS, None)),
        Reason::Compile
    );
}

#[test]
fn two_instances_of_one_version_have_separate_memory() {
    // kai_handle stores the request's first byte at 16 and replies with it.
    let echo =
        "(i32.store8 (i32.const 16) (i32.load8_u (local.get $p))) (i64.const 0x0000001000000001)";
    let wasm = module(echo, "");
    let mut one = load(&wasm, LIMITS, None).unwrap();
    let mut two = load(&wasm, LIMITS, None).unwrap();
    assert_eq!(one.handle(b"a").unwrap(), b"a");
    assert_eq!(two.handle(b"b").unwrap(), b"b");
    assert_eq!(one.export(0).unwrap().unwrap(), b"ak hi");
}

/// In dev builds wasmi's default dispatch grows the native stack per instruction;
/// with portable dispatch a long loop runs on a small stack.
#[test]
fn a_long_loop_runs_on_a_small_native_stack() {
    let thread = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let mut instance = load_ok("(loop $l (br $l)) (unreachable)");
            instance.handle(b"").unwrap_err().reason
        })
        .unwrap();
    assert_eq!(thread.join().unwrap(), Reason::Fuel);
}
