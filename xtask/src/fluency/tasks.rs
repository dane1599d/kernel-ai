//! The 50 fluency-gate tasks. Each says what the script must print and what it may
//! change, and carries a reference script that the unit tests run through the same
//! checks, so a broken checker cannot pass the gate.
//!
//! Do not edit tasks between gate runs: the report records this file's hash.

/// What a task may do to the scheduling policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Must not call `set_policy`.
    Unchanged,
    /// Must switch to exactly this policy.
    Set(&'static str),
    /// May switch to this policy or leave it (it is already this one).
    SetOrKeep(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct Task {
    pub request: &'static str,
    /// Text the output must contain, every piece.
    pub prints: &'static [&'static str],
    pub policy: Policy,
    /// The request is impossible here: the script must say so and change nothing.
    pub refusal: bool,
    pub reference: &'static str,
}

const fn read(
    request: &'static str,
    prints: &'static [&'static str],
    reference: &'static str,
) -> Task {
    Task {
        request,
        prints,
        policy: Policy::Unchanged,
        refusal: false,
        reference,
    }
}

const fn change(
    request: &'static str,
    prints: &'static [&'static str],
    policy: Policy,
    reference: &'static str,
) -> Task {
    Task {
        request,
        prints,
        policy,
        refusal: false,
        reference,
    }
}

const fn refuse(request: &'static str, reference: &'static str) -> Task {
    Task {
        request,
        prints: &[],
        policy: Policy::Unchanged,
        refusal: true,
        reference,
    }
}

/// Phrases a refusal must contain one of, matched as whole words with at most one
/// word between them ("have no" is in "have no network" and "have really no", not in
/// "have now").
pub const REFUSAL_WORDS: &[&str] = &[
    "cannot",
    "can't",
    "can not",
    "unable",
    "not able",
    "not possible",
    "isn't possible",
    "impossible",
    "not supported",
    "isn't supported",
    "not support",
    "doesn't support",
    "not available",
    "unavailable",
    "not allowed",
    "not permitted",
    "no access",
    "no way",
    "no function",
    "not have",
    "don't have",
    "doesn't have",
    "has no",
    "have no",
];

pub const TASKS: &[Task] = &[
    // Single reads.
    read(
        "Tell me the number of processors.",
        &["7"],
        "print(cpus());",
    ),
    read(
        "How much free memory is there, in MiB?",
        &["48"],
        "print(memory().free_mib);",
    ),
    read(
        "What is the total memory in MiB?",
        &["128"],
        "print(memory().total_mib);",
    ),
    read(
        "How long has the system been up, in milliseconds?",
        &["4321"],
        "print(uptime_ms());",
    ),
    read(
        "Which scheduling policy is active?",
        &["interactive"],
        "print(get_policy());",
    ),
    read(
        "How many context switches have happened so far?",
        &["9876"],
        "print(threads().switches);",
    ),
    read(
        "How many services are running?",
        &["3"],
        "print(processes().len());",
    ),
    read(
        "How much kernel heap is in use, in KiB?",
        &["2290"],
        "print(memory().heap_kib);",
    ),
    read(
        "How many thread migrations have there been?",
        &["61"],
        "print(threads().migrations);",
    ),
    read(
        "How many times has the scheduler watchdog ejected a policy?",
        &["13"],
        "print(threads().ejections);",
    ),
    // Reads combined with arithmetic and formatting.
    read(
        "Print free and total memory as free/total MiB.",
        &["48/128"],
        "let m = memory();\nprint(m.free_mib + \"/\" + m.total_mib + \" MiB\");",
    ),
    read(
        "What percentage of memory is in use, rounded down to a whole number?",
        &["62"],
        "let m = memory();\nprint((m.total_mib - m.free_mib) * 100 / m.total_mib);",
    ),
    read(
        "How much memory is in use, in MiB?",
        &["80"],
        "let m = memory();\nprint(m.total_mib - m.free_mib);",
    ),
    read(
        "Print the uptime in whole seconds.",
        &["4"],
        "print(uptime_ms() / 1000);",
    ),
    read(
        "Print the CPU count and the scheduling policy on one line.",
        &["7", "interactive"],
        "print(cpus() + \" CPUs, policy \" + get_policy());",
    ),
    read(
        "How many MiB of free memory is that per CPU, rounded down?",
        &["6"],
        "print(memory().free_mib / cpus());",
    ),
    read(
        "Print the name of the first service in the list.",
        &["shell"],
        "print(processes()[0].name);",
    ),
    read(
        "Print the pids of the first two services.",
        &["12", "25"],
        "let p = processes();\nprint(p[0].pid + \" \" + p[1].pid);",
    ),
    read(
        "Print the uptime in seconds with three decimals, like 1.234.",
        &["4.321"],
        "let ms = uptime_ms();\nprint(ms / 1000 + \".\" + ms % 1000);",
    ),
    read(
        "How many migrations is that per CPU, rounded down?",
        &["8"],
        "print(threads().migrations / cpus());",
    ),
    // Working through lists without loops.
    read(
        "Print the names of all running services.",
        &["shell", "llm", "crasher"],
        "let p = processes();\nprint(p[0].name + \", \" + p[1].name + \", \" + p[2].name);",
    ),
    read(
        "How many characters long is the first service's name?",
        &["5"],
        "print(processes()[0].name.len());",
    ),
    read(
        "Print the last service in the list.",
        &["crasher"],
        "let p = processes();\nprint(p[p.len() - 1].name);",
    ),
    read(
        "Is the second service called llm? Answer yes or no.",
        &["yes"],
        "if processes()[1].name == \"llm\" { print(\"yes\"); } else { print(\"no\"); }",
    ),
    read(
        "Print the sum of the pids of all three services.",
        &["56"],
        "let p = processes();\nprint(p[0].pid + p[1].pid + p[2].pid);",
    ),
    read(
        "Print the name of the service with the highest pid among the three.",
        &["llm"],
        "let p = processes();\nlet best = p[0];\nif p[1].pid > best.pid { best = p[1]; }\nif p[2].pid > best.pid { best = p[2]; }\nprint(best.name);",
    ),
    read(
        "Print the second service's name in upper case.",
        &["LLM"],
        "let name = processes()[1].name;\nname.make_upper();\nprint(name);",
    ),
    read(
        "Print the services and CPUs as a ratio like 2:5.",
        &["3:7"],
        "print(processes().len() + \":\" + cpus());",
    ),
    read(
        "Print the total length of the three service names.",
        &["15"],
        "let p = processes();\nprint(p[0].name.len() + p[1].name.len() + p[2].name.len());",
    ),
    read(
        "Does the first service have a pid below 15? Print true or false.",
        &["true"],
        "print(processes()[0].pid < 15);",
    ),
    // Strings, arrays and maps.
    read(
        "Make a map of the CPU count and the free memory, and print it.",
        &["7", "48"],
        "let m = #{cpus: cpus(), free_mib: memory().free_mib};\nprint(m);",
    ),
    read(
        "Print 'free memory: N MiB' with the real number.",
        &["free memory: 48 MiB"],
        "print(\"free memory: \" + memory().free_mib + \" MiB\");",
    ),
    read(
        "Print the policy name in upper case.",
        &["INTERACTIVE"],
        "let p = get_policy();\np.make_upper();\nprint(p);",
    ),
    read(
        "Print the uptime as minutes and seconds, like 2m5s.",
        &["0m4s"],
        "let s = uptime_ms() / 1000;\nprint(s / 60 + \"m\" + s % 60 + \"s\");",
    ),
    read(
        "Print the scheduler counters: switches, ejections and migrations.",
        &["9876", "13", "61"],
        "let t = threads();\nprint(t.switches + \" \" + t.ejections + \" \" + t.migrations);",
    ),
    read(
        "Is the policy round robin? Print true or false.",
        &["false"],
        "print(get_policy() == \"round-robin\");",
    ),
    read(
        "Print an array holding the CPU count and the service count.",
        &["7", "3"],
        "print([cpus(), processes().len()]);",
    ),
    read(
        "Print used and free memory in MiB as a map.",
        &["80", "48"],
        "let m = memory();\nprint(#{used_mib: m.total_mib - m.free_mib, free_mib: m.free_mib});",
    ),
    read(
        "How many letters are in the policy's name?",
        &["11"],
        "print(get_policy().len());",
    ),
    read(
        "How many digits does the context switch count have?",
        &["4"],
        "print(threads().switches.to_string().len());",
    ),
    // Conditions and changes.
    change(
        "Switch the scheduler to round robin.",
        &[],
        Policy::Set("round-robin"),
        "set_policy(\"round-robin\");",
    ),
    change(
        "If fewer than 8 CPUs are online, use round robin scheduling.",
        &[],
        Policy::Set("round-robin"),
        "if cpus() < 8 { set_policy(\"round-robin\"); }",
    ),
    change(
        "Make sure the scheduler uses the interactive policy, then print the policy.",
        &["interactive"],
        Policy::SetOrKeep("interactive"),
        "if get_policy() != \"interactive\" { set_policy(\"interactive\"); }\nprint(get_policy());",
    ),
    change(
        "Switch to round robin only if more than 100 MiB of memory is free, then print the policy.",
        &["interactive"],
        Policy::Unchanged,
        "if memory().free_mib > 100 { set_policy(\"round-robin\"); }\nprint(\"policy: \" + get_policy());",
    ),
    change(
        "Print the policy, switch to round robin, then print the policy again.",
        &["interactive", "round-robin"],
        Policy::Set("round-robin"),
        "print(get_policy());\nset_policy(\"round-robin\");\nprint(get_policy());",
    ),
    // Things the shell cannot do.
    refuse(
        "Format the disk.",
        "print(\"I cannot format disks from a script.\");",
    ),
    refuse(
        "Open the website example.com.",
        "print(\"I cannot open websites from a script.\");",
    ),
    refuse(
        "Reboot the computer.",
        "print(\"I cannot reboot the computer from a script.\");",
    ),
    refuse(
        "Restart the llm service.",
        "print(\"I cannot restart services from a script.\");",
    ),
    refuse(
        "Read my email.",
        "print(\"I cannot read email: there is no access to it.\");",
    ),
];
