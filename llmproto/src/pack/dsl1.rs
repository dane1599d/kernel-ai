//! `dsl-1`: the Rhai card, the calls every answer may make, worked examples in
//! the tagged call, then one line per row under its group (ADR 0010 §2). Its
//! draft was S10-0's `xtask/tokens/dsl-1-draft.txt`; this renders the same
//! sections from the registry.

use agentabi::{Action, ArgType, RetType, Tier};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
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
script()  // a Rhai program, written next, for what the functions below can read or compute
undo(steps: int 1..1)  // puts back the last change made; undo again redoes it
";

/// A script call carries no code: llm writes it next with the script prompt
/// (`act::local::routed`), and the grammar closes `script` with no arguments.
const SCRIPT: &str = r#"{"name":"script","arguments":{}}"#;

/// (request, the tagged call answering it).
const EXAMPLES: &[(&str, &str)] = &[
    ("how many CPUs are there?", SCRIPT),
    ("what percentage of memory is free?", SCRIPT),
    (
        "switch to round robin",
        r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#,
    ),
    ("print the names of all services", SCRIPT),
    ("how long has it been up, in minutes and seconds?", SCRIPT),
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

/// Families: rows `<family>_<verb>` that take the same arguments and return the
/// same thing share one line, `service_start|stop|restart(name: string)`, with
/// this doc in place of theirs. A family with fewer than two rows present is
/// rendered row by row.
pub(super) const FAMILIES: &[(&str, &str)] = &[
    (
        "service",
        "start runs a stopped one; stop keeps it down; restart resets",
    ),
    (
        "pkg",
        "installs or removes a package; undo puts the old set back",
    ),
    // Every fs family has a HEAD_DOCS line; the group header says where paths
    // start.
    ("fs", "files"),
];

/// A family whose rows share a prefix but not a signature gets its doc from its
/// first row's name here, over [`FAMILIES`]'s for the prefix.
pub(super) const HEAD_DOCS: &[(&str, &str)] = &[
    ("fs_list", "names, text, earlier versions"),
    ("fs_create", "create never overwrites"),
    ("fs_trash", "restore brings it back"),
];

/// Groups after `## system`, by the rows' owner, each under its own header.
const GROUPS: &[(&str, &str)] = &[("fs", "files (paths under /home/user)")];

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

/// `(arg: T, ...) -> R`, what follows the name.
fn signature(row: &Action) -> String {
    let args: alloc::vec::Vec<String> = row
        .args
        .iter()
        .map(|a| format!("{}: {}", a.name, arg_type(&a.ty)))
        .collect();
    let ret = match row.ret {
        RetType::Unit => String::new(),
        ret => format!(" -> {}", ret_type(&ret)),
    };
    format!("({}){ret}", args.join(", "))
}

/// No mark for a read, `undo` for a change that runs, `ask` for one that asks
/// first.
fn mark(row: &Action) -> &'static str {
    match (agentabi::derive(row), row.facts.read_only) {
        (Tier::Run, true) => "",
        (Tier::Run, false) => "undo",
        _ => "ask",
    }
}

/// `name(arg: T, ...) -> R [mark]  // doc`.
pub(super) fn line(row: &Action) -> String {
    let mark = match mark(row) {
        "" => String::new(),
        mark => format!(" [{mark}]"),
    };
    format!("{}{}{mark}  // {}", row.name, signature(row), doc(row))
}

/// The family `row` heads a line for, with its members, or `None` when `row`
/// renders on its own line.
fn family<'a>(row: &Action, rows: &[&'a Action]) -> Option<(&'static str, Vec<&'a Action>)> {
    let (name, doc) = FAMILIES.iter().find(|(name, _)| {
        row.name
            .strip_prefix(name)
            .is_some_and(|v| v.starts_with('_'))
    })?;
    let members: Vec<&Action> = rows
        .iter()
        .copied()
        .filter(|r| {
            r.name
                .strip_prefix(name)
                .is_some_and(|v| v.starts_with('_'))
        })
        .filter(|r| signature(r) == signature(row))
        .collect();
    let doc = HEAD_DOCS
        .iter()
        .find(|(head, _)| members.first().is_some_and(|m| m.name == *head))
        .map_or(*doc, |(_, doc)| doc);
    (members.len() >= 2).then_some((doc, members))
}

/// `service_start|stop|restart(name: string) [undo|ask|ask]  // doc`: one mark
/// when every member has the same.
pub(super) fn family_line(doc: &str, members: &[&Action]) -> String {
    let head = members[0].name;
    let prefix = &head[..=head.find('_').unwrap_or(0)];
    let verbs: Vec<&str> = members
        .iter()
        .map(|r| r.name.strip_prefix(prefix).unwrap_or(r.name))
        .collect();
    let marks: Vec<&str> = members.iter().map(|r| mark(r)).collect();
    let mark = match marks.iter().all(|m| *m == marks[0]) {
        true if marks[0].is_empty() => String::new(),
        true => format!(" [{}]", marks[0]),
        false => format!(" [{}]", marks.join("|")),
    };
    format!(
        "{prefix}{}{}{mark}  // {doc}",
        verbs.join("|"),
        signature(members[0])
    )
}

/// One line per row, a family's rows on the line of its first.
pub(super) fn row_lines(rows: &[&Action]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut done: Vec<&str> = Vec::new();
    for row in rows {
        if done.contains(&row.name) {
            continue;
        }
        match family(row, rows) {
            Some((doc, members)) if members[0].name == row.name => {
                done.extend(members.iter().map(|r| r.name));
                lines.push(family_line(doc, &members));
            }
            _ => lines.push(line(row)),
        }
    }
    lines
}

/// What heads the index lines (ADR 0010 §6).
pub const INDEX_HEADING: &str = "## index (name only; call one and its arguments are shown next)\n";

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
    // A row the index level lists by name is not rendered in full (ADR 0010 §6);
    // the rest are the system's, then each service group's under its header.
    let shown: Vec<&Action> = rows
        .iter()
        .copied()
        .filter(|row| super::indexed(row.name).is_none())
        .collect();
    let grouped = |row: &&&Action| {
        GROUPS
            .iter()
            .any(|(owner, _)| row.owner == agentabi::Owner::Service(owner))
    };
    let system: Vec<&Action> = shown.iter().filter(|r| !grouped(r)).copied().collect();
    for line in row_lines(&system) {
        out.push_str(&line);
        out.push('\n');
    }
    for (owner, header) in GROUPS {
        let group: Vec<&Action> = shown
            .iter()
            .filter(|r| r.owner == agentabi::Owner::Service(owner))
            .copied()
            .collect();
        if group.is_empty() {
            continue;
        }
        let _ = writeln!(out, "## {header}");
        for line in row_lines(&group) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.push_str(INDEX_HEADING);
    for (group, names) in super::INDEX {
        let present: Vec<&str> = names
            .iter()
            .copied()
            .filter(|name| rows.iter().any(|row| row.name == *name))
            .collect();
        if !present.is_empty() {
            let _ = writeln!(out, "{group}: {}", present.join(" "));
        }
    }
    out
}
