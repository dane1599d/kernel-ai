//! `cc` on the host with the pinned cc.wasm: entries at `c>` from the first
//! definition to undo, `:dis`, effects and every way an entry can fail.

mod common;

use common::{Cc, DECLARE};
use sha2::{Digest, Sha256};

#[test]
fn cc_wasm_is_the_pinned_build() {
    let pinned = include_str!("../../third_party/xcc/cc.wasm.sha256");
    let sha: String = Sha256::digest(ccserve::CC_WASM)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(pinned.split_whitespace().next(), Some(sha.as_str()));
}

#[test]
fn definitions_calls_and_values() {
    let mut cc = Cc::open();
    let defs = cc.ok("I64 Sq(I64 x) { return x * x; }");
    assert_eq!(defs.len(), 1);
    assert!(defs[0].starts_with("def fn Sq "), "{defs:?}");
    assert_eq!(cc.value("Sq(4)"), "i64 16");
    assert_eq!(cc.ok("Print(\"hi %d\\n\", 3);"), ["out hi 3\n"]);
    assert_eq!(
        cc.ok("\"x=%lld, %s, %5.2f|\\n\", Sq(5), \"ok\", 2.5;"),
        ["out x=25, ok,  2.50|\n"]
    );
    // A bare string prints itself; a char * value shows as a string.
    assert_eq!(cc.ok("\"hello\""), ["out hello"]);
    assert_eq!(cc.value("(const char *)\"hello\""), "str hello");
    assert_eq!(cc.value("2.5 * 2"), "f64 5");
    assert_eq!(cc.value("3u"), "u64 3");
    // A global with a plain value is assigned where it stands.
    assert_eq!(cc.ok("I64 y = Sq(3);"), ["def var y"]);
    assert_eq!(cc.value("y + 1"), "i64 10");
    // `Name;` calls; a void call with no `;` runs and shows nothing.
    cc.ok("U0 Hi() { Print(\"hello\\n\"); }");
    assert_eq!(cc.ok("Hi;"), ["out hello\n"]);
    assert_eq!(cc.ok("Hi"), ["out hello\n"]);
}

#[test]
fn a_redefinition_switches_callers_and_stored_pointers() {
    let mut cc = Cc::open();
    cc.ok("I64 Sq(I64 x) { return x * x; }");
    cc.ok("I64 Twice() { return Sq(3) * 2; }\nI64 (*fp)(I64) = Sq;");
    assert_eq!(cc.value("Twice()"), "i64 18");
    let redefined = cc.ok("I64 Sq(I64 x) { return x + 100; }");
    let slot = |defs: &[String]| defs[0].rsplit(' ').next().unwrap().to_string();
    assert_eq!(cc.value("Twice()"), "i64 206");
    assert_eq!(cc.value("fp(1)"), "i64 101");
    // A different type gets a new slot; old callers keep the old body.
    let retyped = cc.ok("F64 Sq(F64 x) { return x / 2; }");
    assert!(retyped.contains(&"retyped Sq".to_string()), "{retyped:?}");
    assert_ne!(slot(&retyped), slot(&redefined));
    assert_eq!(cc.value("Sq(3.0)"), "f64 1.5");
    assert_eq!(cc.value("Twice()"), "i64 206");
    // undo goes back one entry at a time, calls included (clang-repl's
    // `%undo`): the two calls, then the retyping.
    let last = cc.seq();
    for seq in [last, last - 1, last - 2] {
        assert_eq!(cc.send("undo"), [format!("undone {seq}")]);
    }
    assert_eq!(cc.value("Sq(3)"), "i64 103");
}

#[test]
fn undo_reverts_slots_and_names() {
    let mut cc = Cc::open();
    cc.ok("I64 F() { return 1; }");
    cc.ok("I64 G() { return F() + 10; }");
    cc.ok("I64 F() { return 2; }");
    assert_eq!(cc.value("G()"), "i64 12");
    cc.send("undo");
    cc.send("undo");
    assert_eq!(cc.value("G()"), "i64 11");
    cc.send("undo");
    cc.send("undo");
    cc.send("undo");
    // F's entry is undone: F is unknown again.
    let gone = cc.eval("F()");
    assert!(gone[0].starts_with("err 1 line 1: "), "{gone:?}");
    assert_eq!(cc.send("undo"), ["err 0 nothing to undo at c>"]);
}

#[test]
fn dis_and_defs() {
    let mut cc = Cc::open();
    cc.ok("I64 Sq(I64 x) { return x * x; }");
    cc.ok("I64 Twice() { return Sq(3) * 2; }");
    let dis = cc.send("dis Twice");
    let text = dis[0].strip_prefix("text ").unwrap();
    assert!(text.starts_with("func Twice\n"), "{text}");
    assert!(text.contains("call Sq (slot "), "{text}");
    assert!(text.contains("i64.mul"), "{text}");
    let kairt = cc.send("dis strlen");
    assert!(kairt[0].starts_with("text func strlen\n"), "{kairt:?}");
    assert_eq!(
        cc.send("dis Nope"),
        ["err 0 Nope is not a function of this session"]
    );
    let defs = cc.send("defs");
    assert_eq!(
        defs,
        ["text fn      I64 Sq(I64 x);\nfn      I64 Twice();\n"]
    );
}

#[test]
fn compile_errors_name_the_entrys_line() {
    let mut cc = Cc::open();
    let err = cc.eval("I64 a = 1;\nI64 b = nope + 1;");
    assert!(err[0].starts_with("err 2 line 2: "), "{err:?}");
    assert!(err[0].contains("nope"), "{err:?}");
    assert_eq!(err.len(), 1);
    // Nothing was defined.
    assert_eq!(cc.send("defs"), ["text "]);
    let shape = cc.eval("x = 1\n#define N 2");
    assert_eq!(shape, ["err 1 a ; is missing at the end of this"]);
}

#[test]
fn a_trap_commits_nothing_and_puts_slots_back() {
    let mut cc = Cc::open();
    cc.ok("I64 Sq(I64 x) { return x * x; }");
    cc.ok("I64 zero = 0;");
    let trapped = cc.eval("I64 Sq(I64 x) { return -1; }\nI64 kept = 5;\nPrint(\"%d\", 1 / zero);");
    assert_eq!(trapped, ["trap div-zero"]);
    assert_eq!(cc.value("Sq(3)"), "i64 9");
    assert!(cc.eval("kept")[0].starts_with("err 1 "));
    assert_eq!(cc.eval("*(I64 *)0x7ffffff0"), ["trap memory"]);
    assert_eq!(
        cc.eval("I64 (*null)(void) = 0;\nnull()"),
        ["trap null-call"]
    );
    // A trap after output keeps the output.
    assert_eq!(
        cc.eval("Print(\"a\");\nzero = 1 / zero;"),
        ["out a", "trap div-zero"]
    );
}

#[test]
fn the_deadline_and_ctrl_c_stop_a_run() {
    let mut cc = Cc::open();
    cc.clock.step_ns.set(1_000_000_000);
    assert_eq!(cc.eval("while (1) {}"), ["trap deadline"]);
    cc.clock.step_ns.set(0);
    cc.clock.interrupted.set(true);
    assert_eq!(cc.eval("for (;;) {}"), ["trap interrupted"]);
    cc.clock.interrupted.set(false);
    assert_eq!(cc.value("1 + 1"), "i64 2");
}

#[test]
fn output_stops_at_six_kib() {
    let mut cc = Cc::open();
    let records = cc.ok("for (I64 i = 0; i < 1000; i++) Print(\"0123456789\");");
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].len(), "out ".len() + 6 * 1024);
    assert_eq!(records[1], "out \n[output cut at 6 KiB]\n");
}

#[test]
fn an_effect_suspends_the_run_until_its_answer() {
    let mut cc = Cc::open();
    cc.ok(DECLARE);
    let asked =
        cc.eval("Print(\"before\\n\");\nI64 r = add_one(41);\nPrint(\"after %lld\\n\", r);\nr");
    assert_eq!(
        asked,
        [
            "out before\n",
            r#"need 1 {"name":"add_one","arguments":{"x":41}}"#
        ]
    );
    let done = cc.send("resume 1 ok\n42");
    assert_eq!(
        done,
        ["out after 42\n", "def var r", "value i64 42", "done"]
    );
    // Text arguments are read from memory; each effect has its own id.
    let asked = cc.eval("note(\"buy milk\", 8)");
    assert_eq!(
        asked,
        [r#"need 2 {"name":"note","arguments":{"text":"buy milk"}}"#]
    );
    let refused = cc.send("resume 2 err\nno");
    assert_eq!(refused, ["value i64 -1", "done"]);
    // A bad pointer is answered at once, without asking.
    assert_eq!(cc.value("note((const char *)0x7ffffff0, 8)"), "i64 -14");
}

#[test]
fn another_request_drops_a_waiting_run() {
    let mut cc = Cc::open();
    cc.ok(DECLARE);
    let asked = cc.eval("I64 Kept() { return 1; }\nadd_one(1);");
    assert!(asked[0].starts_with("need 1 "), "{asked:?}");
    assert_eq!(cc.value("2"), "i64 2");
    assert_eq!(cc.send("resume 1 ok\n2"), ["err 0 no run is waiting"]);
    assert!(cc.eval("Kept()")[0].starts_with("err "));
    // A resume for another effect is refused, and drops the run too: a late
    // answer to an entry's first effect cannot answer its second.
    let asked = cc.eval("add_one(1);\nadd_one(2);");
    assert!(asked[0].starts_with("need 2 "), "{asked:?}");
    assert!(cc.send("resume 2 ok\n2")[0].starts_with("need 3 "));
    assert_eq!(
        cc.send("resume 2 ok\n2"),
        ["err 0 resume names another effect"]
    );
    assert_eq!(cc.send("resume 3 ok\n2"), ["err 0 no run is waiting"]);
}

/// Review findings: effects answered by cc itself still meet the deadline and
/// Ctrl-C, and a reply always ends however many effects a run makes.
#[test]
fn effects_answered_at_once_meet_the_budget() {
    let mut cc = Cc::open();
    cc.ok(DECLARE);
    cc.clock.step_ns.set(100_000_000);
    let reply = cc.eval("for (;;) note((const char *)0x7ffffff0, 1);");
    assert_eq!(reply, ["trap deadline"]);
    cc.clock.step_ns.set(0);
    cc.clock.interrupted.set(true);
    let reply = cc.eval("for (;;) note((const char *)0x7ffffff0, 1);");
    assert_eq!(reply, ["trap interrupted"]);
    cc.clock.interrupted.set(false);
    let many = "for (I64 i = 0; i < 600; i++) { Print(\"x\"); note((const char *)0x7ffffff0, 1); }";
    let reply = cc.ok(many);
    assert_eq!(reply, [format!("out {}", "x".repeat(600))]);
}

#[test]
fn a_trap_puts_the_stack_pointer_back() {
    let mut cc = Cc::open();
    cc.ok("I64 Zero() { return 0; }");
    cc.ok("U0 Boom() { volatile char b[20000]; b[0] = 1; b[1] = (char)(1 / Zero()); }");
    for _ in 0..5 {
        assert_eq!(cc.eval("Boom();"), ["trap div-zero"]);
    }
    assert_eq!(cc.ok("Print(\"%d\\n\", 7);"), ["out 7\n"]);
}

#[test]
fn long_listings_and_names_stay_within_the_wire() {
    let mut cc = Cc::open();
    for block in 0..4 {
        let globals: Vec<String> = (0..300).map(|i| format!("I64 g{block}_{i};")).collect();
        assert_eq!(cc.eval(&globals.join("\n")).last().unwrap(), "done");
    }
    let defs = cc.send("defs");
    assert_eq!(defs.len(), 1);
    assert!(defs[0].len() <= 6 * 1024 + 64, "{}", defs[0].len());
    assert!(
        defs[0].ends_with(" more\n"),
        "{}",
        &defs[0][defs[0].len() - 40..]
    );
    let long = format!("I64 {} = 1;", "a".repeat(64));
    assert_eq!(cc.eval(&long), ["err 1 a name is longer than 63 bytes"]);
}

#[test]
fn imports_are_the_exact_capability_list() {
    let mut cc = Cc::open();
    let foreign = cc.eval(
        "__attribute__((import_module(\"kai\"), import_name(\"fs_trash\"))) I64 fs_trash(I64);\nfs_trash(1);",
    );
    assert_eq!(foreign, ["err 0 import kai.fs_trash is not allowed"]);
    let wasi = cc.eval(
        "__attribute__((import_module(\"wasi_snapshot_preview1\"), import_name(\"proc_exit\"))) void bye(int);\nbye(0);",
    );
    assert_eq!(
        wasi,
        ["err 0 import wasi_snapshot_preview1.proc_exit is not allowed"]
    );
    let typed = cc.eval(
        "__attribute__((import_module(\"kai\"), import_name(\"add_one\"))) I32 add_one(I32);\nadd_one(1);",
    );
    assert_eq!(typed, ["err 0 import kai.add_one is not allowed"]);
}

#[test]
fn replay_restores_definitions_and_runs_nothing() {
    let mut cc = Cc::open();
    assert_eq!(cc.send("open main origin=user"), ["ok"]);
    let replayed =
        cc.send("replay 1\nI64 Sq(I64 x) { return x * x; }\nI64 y = Sq(3);\nPrint(\"ran\");");
    assert_eq!(replayed.last().map(String::as_str), Some("done"));
    assert!(!replayed.iter().any(|r| r.starts_with("out")));
    // Globals reset: y's assignment did not run.
    assert_eq!(cc.value("y"), "i64 0");
    assert_eq!(cc.value("Sq(2)"), "i64 4");
}

#[test]
fn reset_and_sessions() {
    let mut cc = Cc::open();
    cc.ok("I64 Sq(I64 x) { return x * x; }");
    assert_eq!(cc.send("reset"), ["ok"]);
    assert!(cc.eval("Sq(2)")[0].starts_with("err 1 "));
    assert_eq!(
        cc.send("open trial origin=model"),
        ["err 0 model-written C is not enabled"]
    );
    assert_eq!(
        cc.send("build hello"),
        ["err 0 cc build comes with P4 task 13"]
    );
    assert_eq!(cc.send("nonsense"), ["err 0 not a request"]);
}

/// cc's plan line covers everything it may hold at once.
#[test]
fn the_plan_line_covers_the_budget() {
    let pages: u64 = kabi::plan::CC
        .split_whitespace()
        .find_map(|word| word.strip_prefix("pages="))
        .and_then(|n| n.parse().ok())
        .expect("cc's plan line names its pages");
    let budget = ccserve::budget::total();
    assert!(
        budget <= pages * 4096,
        "cc may hold {} KiB, its plan gives {} KiB",
        budget / 1024,
        pages * 4
    );
}
