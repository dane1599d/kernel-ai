// SPDX-License-Identifier: 0BSD
// SPDX-FileCopyrightText: The Kernel AI contributors

//! The worked examples the model sees, and so copies into what it writes for
//! the user (ADR 0009 D1): under 0BSD, so a script or a call it writes from
//! them carries no notice duty. The rest of llmproto is MPL-2.0.

/// The script prompt's examples: (request, script, summary).
pub const SCRIPT: &[(&str, &str, &str)] = &[
    (
        "how many CPUs are there?",
        "print(\"CPUs: \" + cpus());",
        "Prints the number of CPUs.",
    ),
    (
        "what percentage of memory is free?",
        "let m = memory();\nlet pct = m.free_mib * 100 / m.total_mib;\nprint(\"free: \" + pct + \"%\");",
        "Prints free memory as a whole percentage.",
    ),
    (
        "use round robin scheduling unless it is on already",
        "if get_policy() != \"round-robin\" {\n    set_policy(\"round-robin\");\n}\nprint(\"policy: \" + get_policy());",
        "Switches to round-robin if needed and prints the policy.",
    ),
    (
        "print the names of all services",
        "let names = \"\";\nfor p in processes() {\n    if names != \"\" {\n        names += \", \";\n    }\n    names += p.name;\n}\nprint(names);",
        "Prints every service name, separated by commas.",
    ),
    (
        "hello",
        "",
        "Hello! Ask me about this system, or tell me what to do.",
    ),
    (
        "what can you do?",
        "",
        "I answer questions and run scripts that read CPUs, memory, uptime and services, or set the policy. Try: how much memory is free? or use round robin.",
    ),
    (
        "delete all my files",
        "print(\"I cannot do that: scripts here have no access to files.\");",
        "Prints why it cannot be done.",
    ),
];

/// A script call carries no code: llm writes it next with the script prompt
/// (`act::local::routed`), and the grammar closes `script` with no arguments.
const SCRIPT_CALL: &str = r#"{"name":"script","arguments":{}}"#;

/// The act pack's examples: (request, the tagged call answering it).
pub const ACT: &[(&str, &str)] = &[
    ("how many CPUs are there?", SCRIPT_CALL),
    ("what percentage of memory is free?", SCRIPT_CALL),
    (
        "switch to round robin",
        r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#,
    ),
    ("print the names of all services", SCRIPT_CALL),
    (
        "how long has it been up, in minutes and seconds?",
        SCRIPT_CALL,
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
