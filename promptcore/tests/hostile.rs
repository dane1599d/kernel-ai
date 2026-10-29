//! Hostile entries and messages: cut, mutated or bent at every byte, an entry
//! is prepared or refused with a line, never a panic, and a message is read or
//! refused; what reads writes back as the same bytes. Work and memory stay
//! bounded by the input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use promptcore::wire::{Record, Reply, Request, parse_reply};
use promptcore::{Defs, Mode, open};

const ENTRY: &str = "#define N 3\n#include <string.h>\n\
typedef struct { I64 x, y; } Pt;\n\
struct Point { I64 x; } here = {1};\n\
enum Color { RED, GREEN = (1 + 1), BLUE };\n\
static I64 k = 2;\n\
const char *const names[N] = {\"a\", \"b\", \"c\"};\n\
I64 Sq(I64 x) { if (x < 0) { return -x * x; } else return x * x; }\n\
U0 Hi() { for (I64 i = 0; i < N; i++) Print(\"%d\\n\", i); }\n\
I64 y = Sq(3), *p = &y;\n\
do { y--; } while (y > 0);\n\
\"x=%d\\n\", y;\n\
Hi;\n\
/* the end */ Sq(y)";

fn session() -> Defs {
    let mut defs = Defs::new();
    for (seq, entry) in [
        "I64 Twice(I64 x) { return x + x; }",
        "typedef long long Big;",
        "U0 Bye(void) { }",
    ]
    .iter()
    .enumerate()
    {
        let p = defs.prepare(seq as u64, entry, Mode::Eval).unwrap();
        defs.commit(seq as u64, p.defs, &p.drops);
    }
    defs
}

fn entry(defs: &Defs, bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let _ = open(text);
    for mode in [Mode::Eval, Mode::Replay] {
        let (result, peak) = hostile::peak_alloc(|| defs.prepare(9, text, mode));
        if let Ok(p) = &result {
            // The unit is the prelude, the entry's text a few times over, and
            // fixed wrappers.
            let prelude = defs.prelude(&[]).len();
            assert!(p.source.len() <= 64 + prelude + 8 * text.len(), "{text:?}");
            assert!(p.defs.len() <= text.len());
        }
        assert!(
            peak <= 64 * 1024 + 64 * text.len(),
            "{peak} bytes for {} bytes",
            text.len()
        );
    }
}

#[test]
fn the_sample_entry_prepares() {
    let defs = session();
    let p = defs.prepare(9, ENTRY, Mode::Eval).unwrap();
    assert!(p.run.is_some() && p.fallback.is_some());
    assert!(p.defs.len() >= 10, "{:?}", p.defs);
}

#[test]
fn mutated_entries_are_prepared_or_refused() {
    let defs = session();
    hostile::fuzz(ENTRY.as_bytes(), 12, 20_000, |bytes| entry(&defs, bytes));
    hostile::truncations(ENTRY.as_bytes(), |_, cut| entry(&defs, cut));
}

#[test]
fn bent_entries_are_prepared_or_refused() {
    let defs = session();
    let words = [
        "{",
        "}",
        "(",
        ")",
        "[",
        "]",
        ";",
        ",",
        "=",
        "\"",
        "'",
        "/*",
        "*/",
        "#",
        "\\\n",
        "if",
        "else",
        "do",
        "while",
        "typedef",
        "struct",
        "enum",
        "static",
        "const",
        "__attribute__",
        "\n#if 1\n",
        "\n#define\n",
        "\n#include <\n",
        "Hi",
        "Sq",
        "\u{0}",
        "é",
    ];
    let mut rng = hostile::XorShift::new(0x12c);
    for _ in 0..20_000 {
        let mut text = String::from(ENTRY);
        for _ in 0..1 + rng.below(3) {
            let mut at = rng.below(text.len() + 1);
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            text.insert_str(at, words[rng.below(words.len())]);
        }
        entry(&defs, text.as_bytes());
    }
}

#[test]
fn deep_and_long_entries_stay_bounded() {
    let defs = session();
    let max = promptcore::MAX_ENTRY;
    for text in [
        "(".repeat(max),
        "{".repeat(max / 2) + &"}".repeat(max / 2),
        "if (1) ".repeat(max / 7),
        "do ".repeat(max / 3),
        "I64 ".repeat(max / 4),
        "x,".repeat(max / 2),
        "\"".repeat(max),
        "/*".repeat(max / 2),
        "#define A\n".repeat(max / 10),
        "enum { ".to_string() + &"A,".repeat(max / 2 - 8) + "};",
        "struct S { ".to_string() + &"struct S { ".repeat(max / 12),
    ] {
        entry(&defs, text.as_bytes());
    }
}

/// A session filled by many entries undoes them all, each in place.
#[test]
fn many_entries_commit_and_undo() {
    let mut defs = session();
    let start = defs.listing();
    let mut rng = hostile::XorShift::new(5);
    let names = ["a", "b", "c", "Sq", "Twice", "Big", "Pt"];
    let mut committed = 0;
    for seq in 10..400u64 {
        let name = names[rng.below(names.len())];
        let text = match rng.below(4) {
            0 => format!("I64 {name} = {seq};"),
            1 => format!("I64 {name}(I64 x) {{ return x + {seq}; }}"),
            2 => format!("typedef I64 {name};"),
            _ => format!("#define {name} {seq}"),
        };
        let p = defs.prepare(seq, &text, Mode::Eval).unwrap();
        defs.commit(seq, p.defs, &p.drops);
        committed += 1;
    }
    for _ in 0..committed {
        defs.undo().unwrap();
    }
    assert_eq!(defs.listing(), start);
}

const REQUESTS: [&str; 7] = [
    "open main origin=user",
    "eval 12\nI64 Sq(I64 x) {\n  return x * x;\n}",
    "replay 3\nI64 x;",
    "resume 12 ok\n{\"status\":0}",
    "dis Sq",
    "build hello",
    "undo",
];

fn request(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let (result, peak) = hostile::peak_alloc(|| Request::parse(text));
    if let Ok(request) = result {
        assert_eq!(request.text(), text);
    }
    assert!(peak <= 4096 + 2 * text.len());
}

#[test]
fn mutated_requests_are_read_or_refused() {
    for sample in REQUESTS {
        hostile::fuzz(sample.as_bytes(), 3, 5_000, request);
        hostile::truncations(sample.as_bytes(), |_, cut| request(cut));
    }
}

const REPLY: &[u8] = b"out 13\nline 1\nline 2def fn 7 2\nSqdef type - 12\nstruct Pointretyped Sq\nvalue i64 2\n42need 3 9\n{\"a\":\"b\"}";

fn reply(bytes: &[u8]) {
    let (result, peak) = hostile::peak_alloc(|| parse_reply(bytes));
    if let Ok(records) = result {
        let mut again = Reply::new(usize::MAX);
        for record in &records {
            again.push(record).unwrap();
        }
        assert_eq!(again.into_bytes(), bytes);
        assert!(records.last().is_some_and(Record::terminal));
    }
    assert!(peak <= 4096 + 64 * bytes.len());
}

#[test]
fn mutated_replies_are_read_or_refused() {
    assert!(parse_reply(REPLY).is_ok());
    hostile::fuzz(REPLY, 4, 30_000, reply);
    hostile::truncations(REPLY, |_, cut| reply(cut));
    hostile::fuzz(b"out 2\nhidone 1 2 3\n", 5, 10_000, reply);
}

/// Every count and number in a reply's headers, as each interesting value and
/// one past it, in decimal.
#[test]
fn extreme_numbers_are_read_or_refused() {
    let shapes = [
        "out {}\nhiok\n",
        "def fn {} 2\nSqok\n",
        "def fn 7 {}\nSqok\n",
        "need {} 2\n{}",
        "err {} 1\nx",
        "err 1 {}\nx",
        "done {} 0 0\n",
        "undone {}\n",
        "value i64 {}\n42",
    ];
    for shape in shapes {
        for value in hostile::INTERESTING {
            for text in [format!("{value}"), format!("{}", u128::from(value) + 1)] {
                reply(shape.replacen("{}", &text, 1).as_bytes());
            }
        }
    }
}
