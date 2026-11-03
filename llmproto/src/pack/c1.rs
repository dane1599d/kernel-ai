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
     Answer JSON only: {\"script\": \"...\", \"summary\": \"...\"}. summary: one sentence on what it does.\n\
     No program needed (a greeting, a question you can answer): empty script; the summary answers.\n\
     Kai C: statements run in order, no main; kai.h and <string.h> are included. I64 is 64-bit.\n\
     Whole numbers only: / rounds down, percent is x * 100 / y. upper case is c - 32.\n\
     Print is printf: %lld for every number, %s for text, \\n at the end. Print exactly what is asked, in its format, words and order.\n\
     Each call returns a status; its value comes through out: start from its Use line.\n\
     If these functions cannot do it, Print why.\n\
     Functions:\n",
);

/// (request, program, summary): a list, and a change.
const EXAMPLES: &[(&str, &str, &str)] = &[
    (
        "print the names of all services",
        "struct kai_processes_item processes_list[16]; I64 n_processes = processes(processes_list, 16);\nfor (I64 i = 0; i < n_processes; i++) Print(\"%s\\n\", processes_list[i].name);",
        "Prints every service name, one a line.",
    ),
    (
        "use round robin scheduling unless it is on already",
        "char get_policy_text[12]; get_policy(get_policy_text, 12);\nif (strcmp(get_policy_text, \"round-robin\") != 0) set_policy(\"round-robin\");\nPrint(\"policy: round-robin\\n\");",
        "Switches to round-robin if needed.",
    ),
];

/// The card.
pub fn render() -> String {
    let mut card = String::from(RULES);
    for row in agentabi::SCRIPT_API.iter() {
        for line in declare(row).lines() {
            match line.starts_with("struct ") {
                true => {
                    let _ = writeln!(card, "{line}");
                }
                false => {
                    let _ = writeln!(card, "{line}. Use: {}", use_line(row));
                }
            }
        }
    }
    card.push_str("Examples:\n");
    for (request, program, summary) in EXAMPLES {
        let envelope = serde_json::json!({ "script": program, "summary": summary });
        let _ = writeln!(card, "Request: {request}\n{envelope}");
    }
    card
}

/// The template for one call (spike S12-2's re-run): its out variable, named
/// after the call so it does not hide the function, then the call.
pub fn use_line(row: &Action) -> String {
    let name = row.name;
    let mut args: Vec<String> = row
        .args
        .iter()
        .map(|arg| match &arg.ty {
            ArgType::Int { .. } => String::from("1"),
            ArgType::Enum(words) => format!("\"{}\"", words.first().copied().unwrap_or("")),
            _ => String::from("\"text\""),
        })
        .collect();
    let (declared, assigned) = match &row.ret {
        RetType::Of(ArgType::Int { .. }) => {
            args.push(format!("&n_{name}"));
            (format!("I64 n_{name}; "), String::new())
        }
        RetType::Record(_) => {
            args.push(format!("&{name}_now"));
            (format!("struct kai_{name} {name}_now; "), String::new())
        }
        RetType::List(_) => {
            args.push(format!("{name}_list, 16"));
            (
                format!("struct kai_{name}_item {name}_list[16]; "),
                format!("I64 n_{name} = "),
            )
        }
        ret @ (RetType::Of(_) | RetType::Data) => {
            let bytes = text_bytes(ret).unwrap_or(1025);
            args.push(format!("{name}_text, {bytes}"));
            (format!("char {name}_text[{bytes}]; "), String::new())
        }
        _ => (String::new(), String::new()),
    };
    format!("{declared}{assigned}{name}({});", args.join(", "))
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
