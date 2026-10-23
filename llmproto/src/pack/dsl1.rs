//! `dsl-1`: the Rhai card, the calls every answer may make, worked examples in
//! the tagged call, then one line per row under its group (ADR 0010 §2). Its
//! draft was S10-0's `xtask/tokens/dsl-1-draft.txt`; this renders the same
//! sections from the registry.

use agentabi::{Action, ArgType, RetType, Tier};
use alloc::format;
use alloc::string::String;
use core::fmt::Write;

/// Named types, so a row says `Policy` instead of spelling its words again.
const NAMED: &[(&str, &[&str])] = &[("Policy", agentabi::POLICIES)];

const RHAI: &str = "\
## Rhai (for script)
let pct = a * 100 / b;   // integers only: never 1.5; / rounds down; fixed(4321, 3) is \"4.321\"
for x in list { }  for i in 0..n { }   // no while, loop, do, fn, closures or eval
if a > b { print(\"yes\"); } else { print(\"no\"); }   // braces always; end statements with ;
\"a\" + b  print(value)  [1, 2]  a.len()  a[0]  #{a: 1}  m.a  m[\"a\"]  memory().free_mib
string: len() to_upper() to_lower() trim() contains(t) index_of(t) split(t) replace(a, b)
array: len() push(x) pop() contains(x) index_of(x) sort() reverse() join(t)
map: keys() values() contains(k) len()   int: to_string() abs()   floor(n) is n
// Print exactly what is asked, in its order, with its words and units (MiB, KiB).
// If the functions below cannot do it, print why instead.
";

const CALLS: &str = "\
## Calls (no mark: reads; [undo]: runs, undo puts it back; [ask]: asks first)
say(text: string)  // answer in words: a greeting, chat, or a question you can answer
clarify(question: string, options: [string])  // the request has two readings
script(code: string)  // Rhai, at most 40 lines of at most 120 characters
";

/// (request, the tagged call answering it).
const EXAMPLES: &[(&str, &str)] = &[
    (
        "how many CPUs are there?",
        r#"{"name":"script","arguments":{"code":"print(\"CPUs: \" + cpus());"}}"#,
    ),
    (
        "what percentage of memory is free?",
        r#"{"name":"script","arguments":{"code":"let m = memory();\nprint(\"free: \" + m.free_mib * 100 / m.total_mib + \"%\");"}}"#,
    ),
    (
        "switch to round robin",
        r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#,
    ),
    (
        "print the names of all services",
        r#"{"name":"script","arguments":{"code":"let n = [];\nfor p in processes() { n.push(p.name); }\nprint(n);"}}"#,
    ),
    (
        "how long has it been up, in minutes and seconds?",
        r#"{"name":"script","arguments":{"code":"let s = uptime_ms() / 1000;\nprint(s / 60 + \" min \" + s % 60 + \" s\");"}}"#,
    ),
    (
        "hello",
        r#"{"name":"say","arguments":{"text":"Hello! Ask me about this system, or tell me what to do."}}"#,
    ),
    (
        "make it faster",
        r#"{"name":"clarify","arguments":{"question":"Faster how?","options":["interactive policy","round-robin"]}}"#,
    ),
    (
        "delete all my files",
        r#"{"name":"say","arguments":{"text":"I cannot do that: there is no action for files yet."}}"#,
    ),
];

/// Shorter docs for `dsl-1` where the registry's doc is the M8 prompt's (kept byte
/// for byte there) and the line's types already say the rest: S10-0's draft
/// wording. Every name here is a row (a test checks); this table goes once the
/// registry's docs are cut to 60 bytes.
pub(super) const BRIEF: &[(&str, &str)] = &[
    ("cpus", "CPUs online"),
    ("memory", "MiB; kernel heap in KiB"),
    ("uptime_ms", "milliseconds since boot"),
    ("get_policy", "the scheduling policy"),
    ("set_policy", "every CPU"),
    ("tune_undo", "puts back the policy a kept trial replaced"),
    ("component_revert", "a live component back n versions"),
];

fn doc(row: &Action) -> &'static str {
    BRIEF
        .iter()
        .find(|(name, _)| *name == row.name)
        .map_or(row.doc, |(_, doc)| doc)
}

fn named(words: &[&str]) -> Option<&'static str> {
    NAMED
        .iter()
        .find(|(_, these)| *these == words)
        .map(|(name, _)| *name)
}

fn arg_type(ty: &ArgType) -> String {
    match ty {
        ArgType::Enum(words) => named(words).map_or_else(
            || {
                let quoted: alloc::vec::Vec<String> =
                    words.iter().map(|w| format!("\"{w}\"")).collect();
                quoted.join(" | ")
            },
            String::from,
        ),
        ArgType::Int {
            min: i64::MIN,
            max: i64::MAX,
        } => "int".into(),
        ArgType::Int { min, max } => format!("int {min}..{max}"),
        ArgType::Text { .. } => "string".into(),
        ArgType::Path => "path".into(),
        ArgType::Host => "host".into(),
        ArgType::List(inner, _) => format!("[{}]", arg_type(inner)),
        ArgType::Opt(inner) => format!("{}?", arg_type(inner)),
    }
}

fn ret_type(ret: &RetType) -> String {
    match ret {
        RetType::Unit => "()".into(),
        RetType::Of(ty) => arg_type(ty),
        RetType::Data => "data".into(),
        RetType::List(inner) => format!("[{}]", ret_type(inner)),
        RetType::Record(fields) => {
            let fields: alloc::vec::Vec<String> = fields
                .iter()
                .map(|(name, ty)| format!("{name}: {}", ret_type(ty)))
                .collect();
            format!("#{{{}}}", fields.join(", "))
        }
    }
}

/// `name(arg: T, ...) -> R [mark]  // doc`: no mark for a read, `[undo]` for a
/// change that runs, `[ask]` for one that asks first.
pub(super) fn line(row: &Action) -> String {
    let args: alloc::vec::Vec<String> = row
        .args
        .iter()
        .map(|a| format!("{}: {}", a.name, arg_type(&a.ty)))
        .collect();
    let ret = match row.ret {
        RetType::Unit => String::new(),
        ret => format!(" -> {}", ret_type(&ret)),
    };
    let mark = match (agentabi::derive(row), row.facts.read_only) {
        (Tier::Run, true) => "",
        (Tier::Run, false) => " [undo]",
        _ => " [ask]",
    };
    format!(
        "{}({}){ret}{mark}  // {}",
        row.name,
        args.join(", "),
        doc(row)
    )
}

pub(super) fn render(rows: &[&Action]) -> String {
    let mut out = String::from(RHAI);
    out.push_str(CALLS);
    out.push_str("## Examples\n");
    for (request, call) in EXAMPLES {
        let _ = writeln!(out, "> {request}\n{call}");
    }
    out.push_str("## system\n");
    for (name, words) in NAMED {
        let quoted: alloc::vec::Vec<String> = words.iter().map(|w| format!("\"{w}\"")).collect();
        let _ = writeln!(out, "type {name} = {}", quoted.join(" | "));
    }
    for row in rows {
        out.push_str(&line(row));
        out.push('\n');
    }
    out
}
