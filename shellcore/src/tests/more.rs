//! Entries over several lines (`more` replies), Rhai 1.27.0's behaviour the shell
//! relies on, and Grain giving the AST's results.

use super::*;
use crate::brackets::open;

#[test]
fn open_counts_brackets_outside_strings_and_comments() {
    for code in [
        "fn f(x) {",
        "let a = [1, 2,",
        "print((1 + 2)",
        "let m = #{ a: 1,",
        "/* a comment",
        "/* /* nested */ still open",
        "let s = `two",
        "if x { \"}\"",
    ] {
        assert!(open(code), "{code}");
    }
    for code in [
        "",
        "fn f(x) { x }",
        "let s = \"{(\";",
        "let c = '{';",
        "let n = 1; // {",
        "/* { */ 1",
        "let s = `a\nb`",
        "}",
        "1 ) (",
        "\"unclosed",
        "let s = \"a\\\"{\";",
    ] {
        assert!(!open(code), "{code}");
    }
}

#[test]
fn an_open_entry_replies_more_and_runs_when_closed() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let more = shell.eval("fn add(a, b) {");
    assert!(more.more);
    assert_eq!(more.to_wire(), "more");
    assert!(shell.eval("  a + b").more);
    assert_eq!(shell.eval("}"), ok(""));
    assert_eq!(shell.eval("add(2, 3)"), ok("5"));
    assert!(shell.eval("let total = [").more);
    assert!(shell.eval("1,").more);
    assert_eq!(shell.eval("2]; total.len()"), ok("2"));
}

#[test]
fn ctrl_c_drops_an_open_entry_and_a_long_one_is_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(shell.eval("let x = (1 +").more);
    assert_eq!(shell.eval(CANCEL_ENTRY), ok(""));
    assert!(shell.eval("x").text.contains("not found"));
    assert!(shell.eval("let s = [").more);
    let reply = shell.eval(&"1, ".repeat(1400));
    assert!(
        !reply.ok && reply.text.contains("at most 4 KiB"),
        "{reply:?}"
    );
    assert_eq!(shell.eval("1 + 1"), ok("2"));
}

#[test]
fn commands_and_requests_are_not_held_open() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    // A `?` request goes to the model as it is, brackets and all.
    let reply = shell.eval("? what is (this");
    assert!(!reply.more);
}

/// A function typed later replaces the earlier one of the same name and arity, and
/// a typed function shadows a registered one, in Rhai 1.27.0 as in 1.26.1. The
/// model's scripts cannot define functions at all (`fn` is disabled there).
#[test]
fn a_typed_function_redefines_the_earlier_one() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(shell.eval("fn f() { 1 }"), ok(""));
    assert_eq!(shell.eval("fn f() { 2 }"), ok(""));
    assert_eq!(shell.eval("f()"), ok("2"));
    assert_eq!(shell.sizes().1, 1);
    assert_eq!(shell.eval("fn cpus() { 99 }"), ok(""));
    assert_eq!(shell.eval("cpus()"), ok("99"));
}

#[cfg(feature = "grain")]
#[test]
fn grain_gives_the_ast_results() {
    let lines = [
        "let total = 0; for i in 0..100 { total += i * i; } total",
        "fn fib(n) { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }",
        "fib(15)",
        "let m = #{ a: 1, b: [1, 2, 3] }; m.b.len() + m.a",
        "let s = \"\"; for c in \"hello\".chars() { s = c + s; } s",
        "switch 3 { 1 => \"one\", 3 => \"three\", _ => \"many\" }",
        "print(`cpus: ${cpus()}`); 1 / 0",
    ];
    let run = |exec| {
        let host = FakeHost::new();
        let config = Config {
            exec,
            ..DEFAULT_CONFIG
        };
        let mut shell = Shell::new(Rc::clone(&host) as Rc<dyn Host>, config);
        lines.map(|line| shell.eval(line))
    };
    assert_eq!(run(Exec::Ast), run(Exec::Grain));
}

/// S12-3's micro-benchmarks: a sieve and a long sum, each run on both engines. Run
/// with `cargo test -p shellcore --features grain --release -- --ignored --nocapture`;
/// Grain becomes the default only at 1.2x or better on a runner.
#[cfg(feature = "grain")]
#[test]
#[ignore]
fn grain_speed_against_the_ast() {
    let programs = [
        (
            "sieve to 998, 300 times",
            "let n = 998; let count = 0; for round in 0..300 { let s = []; \
             s.pad(n + 1, true); for i in 2..=n { if s[i] { count += 1; let j = i * i; \
             while j <= n { s[j] = false; j += i; } } } } count",
        ),
        (
            "sum 3000000",
            "let t = 0; for i in 0..3000000 { t += i % 7; } t",
        ),
    ];
    for (name, code) in programs {
        let time = |exec| {
            let host = FakeHost::new();
            let config = Config {
                exec,
                line_budget_ms: 60_000,
                ..DEFAULT_CONFIG
            };
            let mut shell = Shell::new(Rc::clone(&host) as Rc<dyn Host>, config);
            let start = std::time::Instant::now();
            let reply = shell.eval(code);
            assert!(reply.ok, "{reply:?}");
            (start.elapsed(), reply.text)
        };
        let (ast, ast_value) = time(Exec::Ast);
        let (grain, grain_value) = time(Exec::Grain);
        assert_eq!(ast_value, grain_value);
        std::println!(
            "{name}: AST {ast:?}, Grain {grain:?}, {:.2}x",
            ast.as_secs_f64() / grain.as_secs_f64()
        );
    }
}
