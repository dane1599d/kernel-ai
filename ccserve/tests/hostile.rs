//! Hostile messages and entries through the whole service: whatever comes in,
//! `cc` answers with one well-formed reply of at most 8 KiB, and its session
//! goes on working.

mod common;
#[path = "../../testkit/hostile.rs"]
mod hostile;

use common::{Cc, DECLARE};
use promptcore::wire::{Record, parse_reply};

fn answer(cc: &mut Cc, message: &[u8]) {
    let reply = cc.service.handle(message, &cc.clock);
    assert!(reply.len() <= ccserve::service::REPLY_CAP);
    let records = parse_reply(&reply)
        .unwrap_or_else(|e| panic!("{:?}: {e}", String::from_utf8_lossy(message)));
    assert!(records.last().is_some_and(Record::terminal));
}

const ENTRY: &str = "eval 9\nstruct P { I64 x, y; };\n\
I64 Sum(struct P *p, I64 n) { I64 s = 0; for (I64 i = 0; i < n; i++) s += p[i].x * p[i].y; return s; }\n\
struct P ps[3] = {{1, 2}, {3, 4}, {5, 6}};\n\
I64 (*f)(struct P *, I64) = Sum;\n\
\"sum %lld\\n\", f(ps, 3);\n\
add_one(Sum(ps, 2))";

#[test]
fn mutated_entries_get_one_reply() {
    let mut cc = Cc::open();
    cc.ok(DECLARE);
    hostile::fuzz(ENTRY.as_bytes(), 0xcc, 300, |bytes| answer(&mut cc, bytes));
    // After all that, the session still works.
    assert_eq!(cc.value("40 + 2"), "i64 42");
}

#[test]
fn every_request_kind_bent() {
    let mut cc = Cc::open();
    let samples: [&[u8]; 8] = [
        b"resume 9 ok\n12",
        b"dis Sum",
        b"undo",
        b"defs",
        b"reset",
        b"build x 0.0.1",
        b"replay 3\nI64 Q(void) { return 1; }",
        b"open main origin=user",
    ];
    for sample in samples {
        hostile::fuzz(sample, 7, 200, |bytes| answer(&mut cc, bytes));
        hostile::truncations(sample, |_, cut| answer(&mut cc, cut));
    }
    answer(&mut cc, b"open main origin=user");
    assert_eq!(cc.value("1"), "i64 1");
}

#[test]
fn hostile_c_stays_inside_the_session() {
    let mut cc = Cc::open();
    for entry in [
        // Writes over its own memory, the stack and kairt's data: allowed, as
        // in C, and contained.
        "for (I64 i = 0; i < 1 << 20; i++) ((char *)0)[i] = 0;",
        "I64 Deep(I64 n) { return Deep(n + 1) + 1; }\nDeep(0)",
        "U8 *p = (U8 *)0x7ffffff0; *p = 1;",
        "I64 (*f)(void) = (I64 (*)(void))12345; f()",
        "__builtin_trap();",
        "__attribute__((import_module(\"env\"), import_name(\"memory\"))) void memory(void);\nmemory();",
        "__attribute__((import_module(\"kai\"), import_name(\"__show_i64\"))) void s(F64);\ns(1);",
    ] {
        let reply = cc.eval(entry);
        assert!(
            reply
                .last()
                .is_some_and(|r| r.starts_with("trap ") || r.starts_with("err ") || r == "done"),
            "{entry}: {reply:?}"
        );
    }
    // The session still answers, though the first entry zeroed kairt's data
    // (ADR 0012: C is memory-unsafe inside its session, never outside it), and
    // `:reset` starts a clean one.
    assert_eq!(cc.value("6 * 7"), "i64 42");
    assert_eq!(cc.send("reset"), ["ok"]);
    assert_eq!(cc.ok("I64 Sq(I64 x) { return x * x; }").len(), 1);
    assert_eq!(cc.ok("Print(\"%lld\\n\", Sq(7));"), ["out 49\n"]);
}
