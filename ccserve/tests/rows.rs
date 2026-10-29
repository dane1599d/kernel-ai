//! The OS's rows as `kai` imports (ADR 0012 D6): the lowering, fixed by a
//! golden; the struct layouts, checked against the compiler's own; and calls
//! from C through `need` and `resume`, with every refusal made before the call
//! is asked about.

mod common;

use ccserve::rows::{KAI_E2BIG, KAI_EFAULT, KAI_EINVAL, KAI_EREFUSED, Rows};
use ccserve::{Effects, Service};
use common::{Clock, show};
use promptcore::wire::parse_reply;

const GOLDEN: &str = "tests/golden/kai_rows.h";

/// The lowering of the OS rows is fixed: a change to it is a change to C
/// code people write, so it shows here first. `CCSERVE_GOLDEN=update`
/// rewrites the file.
#[test]
fn the_lowering_matches_its_golden() {
    let text = Rows::os().declarations();
    if std::env::var("CCSERVE_GOLDEN").as_deref() == Ok("update") {
        std::fs::write(GOLDEN, &text).unwrap();
    }
    let golden = std::fs::read_to_string(GOLDEN)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(
        text, golden,
        "rerun with CCSERVE_GOLDEN=update after checking the change"
    );
}

#[test]
fn the_rows_are_the_ones_the_one_door_runs() {
    let names: Vec<&str> = Rows::os().names().collect();
    assert!(
        names.contains(&"cpus") && names.contains(&"fs_trash") && names.contains(&"pkg_install")
    );
    // Typed-only commands and helpers are not calls: the shell refuses them.
    for absent in [
        "tune",
        "tune_undo",
        "fixed",
        "floor",
        "rules",
        "component_revert",
    ] {
        assert!(!names.contains(&absent), "{absent}");
    }
}

#[test]
fn never_tier_rows_are_left_out() {
    static NEVER: agentabi::Action = agentabi::Action {
        min_tier: agentabi::Tier::Never,
        ..agentabi::CPUS
    };
    assert_eq!(Rows::new(&[&NEVER]).names().count(), 0);
    assert_eq!(Rows::new(&[&agentabi::CPUS]).names().count(), 1);
}

struct Cc {
    service: Service<Rows>,
    clock: Clock,
    seq: u64,
}

impl Cc {
    fn open(rows: Rows) -> Self {
        let clock = Clock::new();
        let service = Service::new(ccserve::CC_WASM, &ccserve::XCC_HEADERS, rows, &clock)
            .expect("cc.wasm loads");
        let mut cc = Self {
            service,
            clock,
            seq: 0,
        };
        assert_eq!(cc.send("open main origin=user"), ["ok"]);
        cc
    }

    fn send(&mut self, message: &str) -> Vec<String> {
        let reply = self.service.handle(message.as_bytes(), &self.clock);
        parse_reply(&reply).unwrap().iter().map(show).collect()
    }

    fn eval(&mut self, entry: &str) -> Vec<String> {
        self.seq += 1;
        self.send(&format!("eval {}\n{entry}", self.seq))
    }
}

/// The compiler lays out each result struct where the host writes it.
#[test]
fn struct_layouts_match_the_compilers() {
    let rows = Rows::os();
    let layouts = rows.layouts();
    assert!(!layouts.is_empty());
    let mut cc = Cc::open(rows);
    assert_eq!(cc.eval("#include <stddef.h>").last().unwrap(), "done");
    for ccserve::rows::Layout { tag, size, fields } in layouts {
        let mut entry = format!("Print(\"%d\", (I32)sizeof(struct {tag}));");
        for (name, _) in &fields {
            entry.push_str(&format!(
                " Print(\" %d\", (I32)offsetof(struct {tag}, {name}));"
            ));
        }
        let mut want = format!("{size}");
        for (_, offset) in &fields {
            want.push_str(&format!(" {offset}"));
        }
        assert_eq!(
            cc.eval(&entry),
            [format!("out {want}"), "done".into()],
            "{tag}"
        );
    }
}

/// The `need` record of a reply, and its id.
fn need(reply: &[String]) -> (u64, String) {
    let last = reply.last().unwrap();
    let rest = last
        .strip_prefix("need ")
        .unwrap_or_else(|| panic!("{reply:?}"));
    let (id, call) = rest.split_once(' ').unwrap();
    (id.parse().unwrap(), call.to_string())
}

#[test]
fn every_result_shape_comes_back_through_c() {
    let mut cc = Cc::open(Rows::os());
    cc.eval(
        "I64 n = 0, st = 0;\nstruct kai_memory m;\nstruct kai_processes_item ps[2];\nchar buf[8];",
    );
    // An Int through `I64 *out`.
    let (id, call) = need(&cc.eval("st = cpus(&n);"));
    assert_eq!(call, r#"{"arguments":{},"name":"cpus"}"#);
    assert_eq!(
        cc.send(&format!("resume {id} ok\n4")).last().unwrap(),
        "done"
    );
    assert_eq!(cc.eval("n")[0], "value i64 4");
    // A record through its struct.
    let (id, _) = need(&cc.eval("st = memory(&m);"));
    let answer = r#"{"free_mib":1,"total_mib":2048,"heap_kib":3}"#;
    cc.send(&format!("resume {id} ok\n{answer}"));
    assert_eq!(cc.eval("m.total_mib + m.heap_kib")[0], "value i64 2051");
    // A list: the status is the full count, the array holds what fits.
    let (id, _) = need(&cc.eval("st = processes(ps, 2);"));
    let answer = r#"[{"pid":1,"name":"init"},{"pid":2,"name":"shell"},{"pid":3,"name":"x"}]"#;
    cc.send(&format!("resume {id} ok\n{answer}"));
    assert_eq!(cc.eval("st")[0], "value i64 3");
    assert_eq!(cc.eval("(const char *)ps[1].name")[0], "value str shell");
    assert_eq!(cc.eval("ps[1].pid")[0], "value i64 2");
    // Text as snprintf fills it: the full length back, the buffer cut with its NUL.
    let (id, call) = need(&cc.eval("st = fs_read(\"/home/user/a.txt\", buf, 8);"));
    assert_eq!(
        call,
        r#"{"arguments":{"path":"/home/user/a.txt"},"name":"fs_read"}"#
    );
    cc.send(&format!("resume {id} ok\n\"hello world\""));
    assert_eq!(cc.eval("st")[0], "value i64 11");
    assert_eq!(cc.eval("(const char *)buf")[0], "value str hello w");
    // A refusal and a failure.
    let (id, _) = need(&cc.eval("fs_trash(\"/home/user/a.txt\")"));
    assert_eq!(
        cc.send(&format!("resume {id} err\nrefused: the user said no"))[0],
        "value i64 -1"
    );
    let (id, _) = need(&cc.eval("fs_trash(\"/home/user/a.txt\")"));
    assert_eq!(
        cc.send(&format!("resume {id} err\nno such file"))[0],
        "value i64 -5"
    );
    // An answer of the wrong shape is a failure, not a write.
    let (id, _) = need(&cc.eval("cpus(&n)"));
    assert_eq!(
        cc.send(&format!("resume {id} ok\n\"four\""))[0],
        "value i64 -5"
    );
}

#[test]
fn bad_arguments_are_refused_before_asking() {
    let mut cc = Cc::open(Rows::os());
    let long = "a".repeat(300);
    for (entry, status) in [
        ("cpus((I64 *)0x7ffffff0)", KAI_EFAULT),
        ("cpus((I64 *)0)", KAI_EFAULT),
        ("fs_trash(NULL)", KAI_EFAULT),
        ("fs_trash((const char *)0x7ffffff0)", KAI_EFAULT),
        (&format!("fs_trash(\"{long}\")"), KAI_E2BIG),
        ("set_policy(\"fastest\")", KAI_EINVAL),
        ("char b[4]; fs_read(\"/a\", b, -1)", KAI_EINVAL),
        ("fs_read(\"/a\", (char *)0x7ffffff0, 64)", KAI_EFAULT),
        (
            "processes((struct kai_processes_item *)16, 1LL << 40)",
            KAI_EFAULT,
        ),
    ] {
        let reply = cc.eval(entry);
        let value = reply.iter().find(|r| r.starts_with("value "));
        assert_eq!(
            value,
            Some(&format!("value i64 {status}")),
            "{entry}: {reply:?}"
        );
    }
    // A good word is asked about; a zero-length buffer may be NULL.
    assert!(cc.eval("set_policy(\"interactive\")")[0].starts_with("need "));
    assert!(cc.eval("fs_read(\"/a\", NULL, 0)")[0].starts_with("need "));
    let _ = KAI_EREFUSED;
}

#[test]
fn ranges_and_optional_arguments() {
    // `fixed` and `tune` are not OS calls; lowered here to test Int ranges
    // and an optional text argument.
    let rows = Rows::new(&[&agentabi::FIXED, &agentabi::TUNE]);
    let mut cc = Cc::open(rows);
    cc.eval("char b[16];");
    assert_eq!(
        cc.eval("fixed(123, 10, b, 16)")[0],
        format!("value i64 {KAI_EINVAL}")
    );
    let (_, call) = need(&cc.eval("fixed(123, 2, b, 16)"));
    assert_eq!(call, r#"{"arguments":{"n":123,"places":2},"name":"fixed"}"#);
    let declared = Rows::new(&[&agentabi::TUNE]).declarations();
    let proto = declared
        .lines()
        .find(|l| l.contains("I64 tune("))
        .unwrap()
        .to_string();
    // Every optional text argument passed as NULL is left out of the call.
    let nulls = proto.matches("const char *").count();
    let args = vec!["NULL"; nulls].join(", ");
    let (_, call) = need(&cc.eval(&format!("tune({args})")));
    assert_eq!(call, r#"{"arguments":{},"name":"tune"}"#, "{proto}");
}

#[test]
fn hostile_calls_and_answers_never_panic() {
    let rows = Rows::os();
    let imports = rows.imports();
    let mut rng: u64 = 0x5eed;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let answers = [
        "4",
        "-1",
        "\"text\"",
        "{\"pid\":1}",
        "[1,2,3]",
        "[{\"name\":5}]",
        "null",
        "{",
        "18446744073709551616",
        "\"\u{e9}\u{e9}\u{e9}\"",
    ];
    for _ in 0..20_000 {
        let (name, ty) = &imports[(next() % imports.len() as u64) as usize];
        let mut memory = vec![0u8; 4096];
        for b in memory.iter_mut().take(256) {
            *b = next() as u8;
        }
        let args: Vec<u64> = ty
            .params
            .iter()
            .map(|_| match next() % 4 {
                0 => next() % 4200,
                1 => next() % 300,
                2 => u64::MAX - (next() % 4),
                _ => next(),
            })
            .collect();
        let _ = rows.call(name, &args, &memory);
        let answer = answers[(next() % answers.len() as u64) as usize];
        let status = rows.answer(name, &args, next() % 2 == 0, answer, &mut memory);
        assert!(status >= -22, "{name} {status}");
        assert_eq!(memory.len(), 4096);
    }
}
