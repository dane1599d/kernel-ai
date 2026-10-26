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

const PANIC_IMPORT: &str = r#"(import "kai" "panic" (func $panic (param i32 i32 i32 i32)))"#;

/// A module whose `kai_handle` announces a panic at `path:line:column` (the path
/// stored at 64) when the request starts with `p` and then traps; for `r` it
/// announces one and replies after all; anything else traps bare.
fn panics_at(path: &str, line: i32, column: i32) -> Vec<u8> {
    let announce = std::format!(
        "(call $panic (i32.const 64) (i32.const {}) (i32.const {line}) (i32.const {column}))",
        path.len()
    );
    let handle = std::format!(
        "(if (i32.eq (i32.load8_u (local.get $p)) (i32.const 114))
           (then {announce} (return {REPLY})))
         (if (i32.eq (i32.load8_u (local.get $p)) (i32.const 112))
           (then {announce}))
         (unreachable)"
    );
    let data = std::format!("(data (i32.const 64) \"{}\")", path.replace('\\', "\\\\"));
    module_with(PANIC_IMPORT, &handle, &data)
}

#[test]
fn a_panic_names_its_file_and_line() {
    let wasm = panics_at("components\\notes\\src\\lib.rs", 58, 47);
    let mut instance = load(&wasm, LIMITS, Some(sink)).unwrap();
    let fault = instance.handle(b"p").unwrap_err();
    assert_eq!(
        (fault.reason, fault.detail.as_str()),
        (
            Reason::Trap,
            "panicked at components/notes/src/lib.rs:58:47"
        )
    );
    // The location belongs to its call only: announced in a call that then replies,
    // it says nothing of a bare trap in the next one.
    assert_eq!(instance.handle(b"r").unwrap(), b"ok hi");
    let bare = instance.handle(b"x").unwrap_err();
    assert_eq!(bare.reason, Reason::Trap);
    assert!(!bare.detail.starts_with("panicked at"), "{}", bare.detail);
    // A rehearsal runs quiet, and still learns where it panicked.
    instance.quiet(true);
    let quiet = instance.handle(b"p").unwrap_err();
    assert!(quiet.detail.starts_with("panicked at components/notes/"));
}

#[test]
fn a_host_path_in_a_panic_is_cut_to_its_file_name() {
    let registry = "C:\\Users\\someone\\.cargo\\registry\\src\\minicbor-2.3.0\\src\\decode.rs";
    let wasm = panics_at(registry, 120, 9);
    let fault = load(&wasm, LIMITS, None).unwrap().handle(b"p").unwrap_err();
    assert_eq!(fault.detail, "panicked at .../decode.rs:120:9");
    // Of a path over 256 bytes the end is read, so the name is still the file's.
    let deep = std::format!(
        "/home/ci/{}/crates/some-deep-crate/src/decoder/frames.rs",
        "x".repeat(250)
    );
    let wasm = panics_at(&deep, 10, 5);
    let fault = load(&wasm, LIMITS, None).unwrap().handle(b"p").unwrap_err();
    assert_eq!(fault.detail, "panicked at .../frames.rs:10:5");
}

#[test]
fn a_panic_path_outside_memory_leaves_the_plain_trap() {
    let handle =
        "(call $panic (i32.const -16) (i32.const 8) (i32.const 3) (i32.const 1)) (unreachable)";
    let wasm = module_with(PANIC_IMPORT, handle, "");
    let fault = load(&wasm, LIMITS, None).unwrap().handle(b"").unwrap_err();
    assert_eq!(fault.reason, Reason::Trap);
    assert!(!fault.detail.starts_with("panicked at"), "{}", fault.detail);
}

/// A canonical-ABI module: `squares(n: u32) -> list<u32>` through a return area,
/// a bump `cabi_realloc`, and `spin()`, a loop that never ends.
const CANONICAL: &str = r#"(module
  (memory (export "memory") 1)
  (global $top (mut i32) (i32.const 1024))
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (local $at i32)
    global.get $top
    local.set $at
    global.get $top
    local.get 3
    i32.add
    global.set $top
    local.get $at)
  (func (export "squares") (param $n i32) (result i32)
    (local $i i32)
    (block $done
      (loop $next
        local.get $i
        local.get $n
        i32.ge_u
        br_if $done
        local.get $i
        i32.const 4
        i32.mul
        i32.const 64
        i32.add
        local.get $i
        local.get $i
        i32.mul
        i32.store
        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $next))
    i32.const 0
    i32.const 64
    i32.store
    i32.const 4
    local.get $n
    i32.store
    i32.const 0)
  (func (export "spin") (loop $l br $l)))"#;

#[test]
fn a_canonical_module_answers_through_appabi() {
    let mut guest = load_core(&wat::parse_str(CANONICAL).unwrap(), LIMITS, None).unwrap();
    let func = appabi::Func {
        export: String::from("squares"),
        params: alloc::vec![appabi::Ty::U32],
        result: Some(appabi::Ty::List(alloc::boxed::Box::new(appabi::Ty::U32))),
    };
    let value = appabi::call(&mut guest, &func, &[appabi::Val::U32(5)]).unwrap();
    let squares: Vec<appabi::Val> = [0u32, 1, 4, 9, 16].map(appabi::Val::U32).to_vec();
    assert_eq!(value, Some(appabi::Val::List(squares)));
    use appabi::Guest;
    assert_eq!(guest.realloc(0, 0, 4, 16).unwrap(), 1024);
    assert!(guest.read(65_530, 10).is_err());
    assert!(Guest::call(&mut guest, "missing", &[]).is_err());
}

#[test]
fn every_call_gets_its_own_fuel() {
    let mut guest = load_core(&wat::parse_str(CANONICAL).unwrap(), LIMITS, None).unwrap();
    use appabi::Guest;
    assert!(Guest::call(&mut guest, "spin", &[]).is_err());
    // The loop spent its call's fuel, not the next call's.
    assert!(Guest::call(&mut guest, "squares", &[appabi::Core::I32(3)]).is_ok());
}

#[test]
fn a_canonical_module_with_imports_is_refused_for_now() {
    let wasm = wat::parse_str(
        r#"(module (import "kai:app/log" "write" (func)) (memory (export "memory") 1))"#,
    )
    .unwrap();
    assert_eq!(reason(load_core(&wasm, LIMITS, None)), Reason::Link);
}

/// The view guest `appbench` runs (components/fixtures/viewbench), when it has been
/// built for wasm32: 300 nodes come back, lifted by `appabi`.
#[test]
fn the_view_guest_rebuilds_300_nodes() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-unknown-unknown/component/comp_viewbench.wasm"
    );
    let Ok(wasm) = std::fs::read(path) else {
        std::eprintln!("skipped: build comp-viewbench for wasm32 first");
        return;
    };
    let mut guest = load_core(&wasm, LIMITS, None).unwrap();
    use appabi::{Func, Ty, Val};
    let s = String::from;
    let node = Ty::Record(alloc::vec![
        (s("id"), Ty::U32),
        (s("parent"), Ty::U32),
        (
            s("kind"),
            Ty::Enum(alloc::vec![s("root"), s("header"), s("row"), s("footer")])
        ),
        (
            s("rect"),
            Ty::Record(alloc::vec![
                (s("x"), Ty::U32),
                (s("y"), Ty::U32),
                (s("w"), Ty::U32),
                (s("h"), Ty::U32)
            ])
        ),
        (s("text"), Ty::String),
    ]);
    let rebuild = Func {
        export: s("rebuild"),
        params: alloc::vec![Ty::U32],
        result: Some(Ty::List(alloc::boxed::Box::new(node))),
    };
    for event in [0u32, 7, 999] {
        let Some(Val::List(nodes)) =
            appabi::call(&mut guest, &rebuild, &[Val::U32(event)]).unwrap()
        else {
            panic!("not a list");
        };
        assert_eq!(nodes.len(), 300);
        let Val::Record(header) = &nodes[1] else {
            panic!()
        };
        assert_eq!(header[4], Val::String(s("Notes: 1000")));
        assert_eq!(header[2], Val::Enum(1));
    }
    let layout = Func {
        export: s("layout"),
        params: alloc::vec![Ty::U32, Ty::U32],
        result: Some(Ty::List(alloc::boxed::Box::new(Ty::String))),
    };
    let Some(Val::List(rows)) =
        appabi::call(&mut guest, &layout, &[Val::U32(240), Val::U32(67)]).unwrap()
    else {
        panic!("not a list");
    };
    assert_eq!(rows.len(), 67);
    assert!(
        rows.iter()
            .all(|r| matches!(r, Val::String(t) if t.len() <= 240))
    );
}

/// A `lifecycle 2` version in WAT: `kai:live/lifecycle@0.1.0` on the canonical ABI,
/// logging through `kai:app/log` and reading `kai:app/clock`. `save` returns the
/// 3 bytes "abc" (or none for schema 9); `migrate` fails with `bad-state` on empty
/// state; `health` returns the clock's seed.
fn lifecycle2(extra_imports: &str) -> Vec<u8> {
    let text = std::format!(
        r#"(module
  (import "kai:app/log@0.1.0" "write" (func $log (param i32 i32)))
  (import "kai:app/clock@0.1.2" "seed" (func $seed (result i64)))
  {extra_imports}
  (memory (export "memory") 1)
  (data (i32.const 16) "abc")
  (data (i32.const 32) "hello")
  (global $top (mut i32) (i32.const 4096))
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (local $at i32)
    global.get $top
    local.set $at
    global.get $top
    local.get 3
    i32.add
    global.set $top
    local.get $at)
  (func (export "kai:live/lifecycle@0.1.0#schema") (result i32)
    i32.const 32
    i32.const 5
    call $log
    i32.const 3)
  (func (export "kai:live/lifecycle@0.1.0#init") (result i32)
    i32.const 64
    i32.const 0
    i32.store8
    i32.const 64)
  (func (export "kai:live/lifecycle@0.1.0#migrate") (param i32 i32 i32) (result i32)
    i32.const 64
    local.get 1
    i32.eqz
    i32.store8
    i32.const 65
    i32.const 1
    i32.store8
    i32.const 64)
  (func (export "kai:live/lifecycle@0.1.0#save") (param i32) (result i32)
    i32.const 80
    local.get 0
    i32.const 9
    i32.ne
    i32.store8
    i32.const 84
    i32.const 16
    i32.store
    i32.const 88
    i32.const 3
    i32.store
    i32.const 80)
  (func (export "kai:live/lifecycle@0.1.0#health") (result i32)
    call $seed
    i32.wrap_i64)
  (func (export "kai:live/lifecycle@0.1.0#self-test") (result i32)
    i32.const 0))"#
    );
    wat::parse_str(&text).unwrap()
}

#[test]
fn a_lifecycle_2_version_serves_the_lifecycle() {
    LOGGED.lock().unwrap().clear();
    let mut v = load_core(&lifecycle2(""), LIMITS, Some(sink)).unwrap();
    assert_eq!(v.schema(), Ok(3));
    assert!(LOGGED.lock().unwrap().iter().any(|l| l == "hello"));
    assert_eq!(v.init(), Ok(0));
    assert_eq!(v.import(b"state", 3), Ok(0));
    assert_eq!(v.import(b"", 3), Ok(2), "bad-state is case 1, so 2");
    assert_eq!(v.export(0), Ok(Some(b"abc".to_vec())));
    assert_eq!(v.export(9), Ok(None));
    v.set_call_inputs(1_000, 42);
    assert_eq!(v.health(), Ok(42));
    assert_eq!(v.self_test(), Ok(0));
    // With no action rows, a request is refused in the reply, not a fault.
    let reply = v.handle(b"count").unwrap();
    assert!(
        reply.starts_with(
            b"kai-reply 1
refused "
        ),
        "{reply:?}"
    );
}

#[test]
fn lifecycle_2_imports_link_only_by_name_type_and_version() {
    let storage = r#"(import "kai:app/storage@0.1.0" "delete" (func (param i32 i32)))"#;
    assert_eq!(
        reason(load_core(&lifecycle2(storage), LIMITS, None)),
        Reason::Link
    );
    let wrong_type = r#"(import "kai:app/log@0.1.0" "panic" (func (param i32)))"#;
    assert_eq!(
        reason(load_core(&lifecycle2(wrong_type), LIMITS, None)),
        Reason::Link
    );
    let newer = r#"(import "kai:app/log@0.2.0" "write" (func (param i32 i32)))"#;
    assert_eq!(
        reason(load_core(&lifecycle2(newer), LIMITS, None)),
        Reason::Link
    );
    let unknown = r#"(import "kai:app/log@0.1.0" "shout" (func (param i32 i32)))"#;
    assert_eq!(
        reason(load_core(&lifecycle2(unknown), LIMITS, None)),
        Reason::Link
    );
}

#[test]
fn load_live_picks_the_lifecycle_from_the_manifest() {
    // A raw module has no manifest: lifecycle 1.
    assert!(matches!(
        load_live(&module("i64.const 0", ""), LIMITS, None),
        Ok(LiveInstance::V1(_))
    ));
    // A lifecycle 1 module named as lifecycle 2 does not link as one.
    let manifest = livecore::artifact::Manifest {
        lifecycle: 2,
        component: String::from("probe"),
        version: String::from("1"),
        world: String::from("app:probe/probe@1.0.0"),
        engine: String::from("wasmi"),
        features: String::from("-"),
        licence: String::from("MPL-2.0"),
        build: String::from("ab12"),
        source: String::from("cd34"),
    };
    let mut artifact = lifecycle2("");
    use livecore::artifact::{append_custom_section, bundle};
    append_custom_section(
        &mut artifact,
        kabi::live::MANIFEST_SECTION,
        manifest.format().as_bytes(),
    );
    append_custom_section(
        &mut artifact,
        kabi::live::SOURCE_SECTION,
        &bundle(&[("src/lib.wat", b"")]),
    );
    // Lifecycle 2 needs its action rows.
    assert_eq!(reason(load_live(&artifact, LIMITS, None)), Reason::Link);
    let rows = "kai-actions 1
world app:probe/probe@1.0.0
action probe_ping
export app:probe/actions@1.0.0#ping
doc Ping.
sig func()
inverse none
end
";
    let mut bad = artifact.clone();
    append_custom_section(
        &mut bad,
        kabi::live::ACTIONS_SECTION,
        b"kai-actions 1
",
    );
    assert_eq!(reason(load_live(&bad, LIMITS, None)), Reason::Link);
    append_custom_section(&mut artifact, kabi::live::ACTIONS_SECTION, rows.as_bytes());
    let mut live = load_live(&artifact, LIMITS, None).unwrap();
    assert!(matches!(live, LiveInstance::V2(_)));
    assert_eq!(live.schema(), Ok(3));
    // A call the rows do not declare is refused; one they do reaches the module,
    // which has no such export: the version's fault.
    let other = live
        .handle(
            b"kai-call 1
func app:probe/actions@1.0.0#pong
args {}
",
        )
        .unwrap();
    assert!(other.starts_with(
        b"kai-reply 1
refused "
    ));
    assert!(
        live.handle(
            b"kai-call 1
func app:probe/actions@1.0.0#ping
args {}
"
        )
        .is_err()
    );
}
