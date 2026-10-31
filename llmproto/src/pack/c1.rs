//! `c-1` (ADR 0012, spike S12-2): the Kai C card for program requests. The
//! model writes one `c>` entry: C statements run top to bottom against kai.h.
//! It shows the rows a script may call (`agentabi::SCRIPT_API`), lowered as
//! ccserve lowers them into kai.h (`ccserve::rows`); xtask's test holds every
//! prototype here to kai.h's, byte for byte. The answer comes in the script
//! envelope, so the same JSON shape and limits decode it.

use agentabi::{Action, ArgType, RetType};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// The rules before the calls.
const RULES: &str = concat!(
    "You write short Kai C programs for the c> prompt of ",
    kabi::product_name!(),
    ", a small operating system.\n\
     Answer with JSON only: {\"script\": \"...\", \"summary\": \"...\"}.\n\
     script: Kai C, at most 40 lines of at most 120 characters, ASCII only.\n\
     summary: for a program, one sentence of at most 100 characters saying what it does.\n\
     No program needed (greeting, chat, a question you can answer): an empty script, and the summary is your answer to the user, at most 300 characters.\n\
     \n\
     Kai C rules here:\n\
     - The program is statements run top to bottom, as at a prompt: no main.\n\
     - kai.h is already included. I64 is a 64-bit integer.\n\
     - For strlen, strcmp or the rest of <string.h>, the first line is #include <string.h>\n\
     - Print(fmt, ...) is printf: %lld for an I64, %s for text. End each line with \\n.\n\
     - Each call below returns a status, negative on failure, and gives its value through out:\n\
       I64 n; cpus(&n); then n is the CPU count.\n\
     - Text comes back into a buffer you pass: char p[32]; get_policy(p, 32);\n\
     - A list fills an array and returns the count: struct kai_processes_item s[16]; I64 count = processes(s, 16);\n\
     - Integers only. / rounds down (7 / 2 is 3). Percent: x * 100 / y.\n\
     - uptime_ms is milliseconds: seconds are ms / 1000; of s seconds, s / 60 are minutes, s % 60 seconds.\n\
     - Compare text with strcmp(a, b) == 0. No <ctype.h>: upper case is c - 32 for 'a' to 'z'.\n\
     - Call only the functions below, Print and <string.h>.\n\
     - Print exactly what is asked, in its format, with its words and units (MiB, KiB).\n\
     - Do the steps in the order asked: print first if the request prints first.\n\
     - If the request cannot be done with these functions, Print why instead.\n\
     \n\
     Functions:\n",
);

/// (request, program, summary): one read, a list, a change, and words.
const EXAMPLES: &[(&str, &str, &str)] = &[
    (
        "what percentage of memory is free?",
        "struct kai_memory m;\nmemory(&m);\nPrint(\"free: %lld%%\\n\", m.free_mib * 100 / m.total_mib);",
        "Prints free memory as a whole percentage.",
    ),
    (
        "print the names of all services",
        "struct kai_processes_item s[16];\nI64 count = processes(s, 16);\nfor (I64 i = 0; i < count; i++) {\n    Print(\"%s\\n\", s[i].name);\n}",
        "Prints every service name, one a line.",
    ),
    (
        "use round robin scheduling unless it is on already",
        "#include <string.h>\nchar p[32];\nget_policy(p, 32);\nif (strcmp(p, \"round-robin\") != 0) set_policy(\"round-robin\");\nget_policy(p, 32);\nPrint(\"policy: %s\\n\", p);",
        "Switches to round-robin if needed and prints the policy.",
    ),
    (
        "hello",
        "",
        "Hello! Ask me about this system, or tell me what to do.",
    ),
    (
        "delete all my files",
        "Print(\"I cannot do that: programs here have no access to files.\\n\");",
        "Prints why it cannot be done.",
    ),
];

/// The card.
pub fn render() -> String {
    let mut card = String::from(RULES);
    for row in agentabi::SCRIPT_API.iter() {
        card.push_str(&declare(row));
    }
    card.push_str("\nExamples:\n");
    for (request, program, summary) in EXAMPLES {
        let envelope = serde_json::json!({ "script": program, "summary": summary });
        let _ = writeln!(card, "Request: {request}\n{envelope}");
    }
    card
}

/// `row`'s struct, if it has one, and its prototype with its doc, as kai.h
/// declares them (without the import attribute).
pub fn declare(row: &Action) -> String {
    let name = row.name;
    let mut out = String::new();
    let tag = match &row.ret {
        RetType::Record(fields) => Some((format!("kai_{name}"), *fields)),
        RetType::List(RetType::Record(fields)) => Some((format!("kai_{name}_item"), *fields)),
        _ => None,
    };
    if let Some((tag, fields)) = &tag {
        let _ = write!(out, "struct {tag} {{");
        for (field, ty) in fields.iter() {
            match text_bytes(ty) {
                None => {
                    let _ = write!(out, " I64 {field};");
                }
                Some(bytes) => {
                    let _ = write!(out, " char {field}[{bytes}];");
                }
            }
        }
        let _ = writeln!(out, " }};");
    }
    let mut params: Vec<String> = row
        .args
        .iter()
        .map(|arg| match arg.ty {
            ArgType::Int { .. } => format!("I64 {}", arg.name),
            _ => format!("const char *{}", arg.name),
        })
        .collect();
    match (&row.ret, &tag) {
        (RetType::Of(ArgType::Int { .. }), _) => params.push("I64 *out".into()),
        (RetType::Of(_) | RetType::Data, _) => params.push("char *out, I64 cap".into()),
        (RetType::Record(_), Some((tag, _))) => params.push(format!("struct {tag} *out")),
        (RetType::List(_), Some((tag, _))) => params.push(format!("struct {tag} *out, I64 cap")),
        _ => {}
    }
    let params = match params.is_empty() {
        true => String::from("void"),
        false => params.join(", "),
    };
    let _ = writeln!(out, "I64 {name}({params}); // {}", row.doc);
    out
}

/// A text field's `char` array size, its NUL included; `None` for an `I64`.
fn text_bytes(ty: &RetType) -> Option<usize> {
    match ty {
        RetType::Of(ArgType::Text { max }) => Some(usize::from(*max) + 1),
        RetType::Of(ArgType::Enum(words)) => {
            Some(words.iter().map(|w| w.len()).max().unwrap_or(0) + 1)
        }
        RetType::Of(ArgType::Path) | RetType::Data => Some(1025),
        RetType::Of(ArgType::Host) => Some(254),
        _ => None,
    }
}
