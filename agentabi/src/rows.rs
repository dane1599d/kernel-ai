//! The OS rows of `REGISTRY_VERSION` 1: the seven M8 script functions, the shell's
//! two pure helpers, and the typed `tune`, `tune undo` and `:revert`.

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};
use kabi::cap;

const INT: ArgType = ArgType::Int {
    min: i64::MIN,
    max: i64::MAX,
};
const STRING: ArgType = ArgType::Text { max: 64 };
/// The policies `set_policy` takes.
pub const POLICIES: &[&str] = &["interactive", "round-robin"];
/// Processes no row may act on: the shell, the supervisor and the llm service
/// (`shellcore/src/tune.rs`, `kernel/src/sched_control.rs`).
pub const PROTECTED: &[&str] = &["shell", "supervisor", "llm"];

/// A read of system state with no arguments.
const fn read(name: &'static str, doc: &'static str, ret: RetType) -> Action {
    Action {
        name,
        doc,
        args: &[],
        facts: Facts::LOCAL_READ,
        inverse: Inverse::Read,
        cues: &[],
        returns: Returns::System,
        ret,
        owner: Owner::Kernel,
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

/// A pure function of its arguments, run by the shell.
const fn helper(
    name: &'static str,
    doc: &'static str,
    args: &'static [Arg],
    ret: RetType,
) -> Action {
    Action {
        args,
        owner: Owner::Shell,
        ..read(name, doc, ret)
    }
}

const fn arg(name: &'static str, ty: ArgType, role: Role) -> Arg {
    Arg {
        name,
        ty,
        role,
        from_request: false,
        never: &[],
        aliases: &[],
    }
}

pub const CPUS: Action = read("cpus", "number of CPUs online", RetType::Of(INT));

pub const MEMORY: Action = read(
    "memory",
    "free and total memory in MiB, and the kernel heap in use in KiB",
    RetType::Record(&[
        ("free_mib", RetType::Of(INT)),
        ("total_mib", RetType::Of(INT)),
        ("heap_kib", RetType::Of(INT)),
    ]),
);

pub const UPTIME_MS: Action = read(
    "uptime_ms",
    "milliseconds since boot (1000 ms is 1 s)",
    RetType::Of(INT),
);

pub const PROCESSES: Action = read(
    "processes",
    "the running services",
    RetType::List(&RetType::Record(&[
        ("pid", RetType::Of(INT)),
        ("name", RetType::Of(STRING)),
    ])),
);

pub const THREADS: Action = read(
    "threads",
    "scheduler counters since boot",
    RetType::Record(&[
        ("switches", RetType::Of(INT)),
        ("ejections", RetType::Of(INT)),
        ("migrations", RetType::Of(INT)),
    ]),
);

pub const GET_POLICY: Action = read(
    "get_policy",
    "the scheduling policy, \"interactive\" or \"round-robin\"",
    RetType::Of(ArgType::Enum(POLICIES)),
);

pub const SET_POLICY: Action = Action {
    name: "set_policy",
    doc: "switches every CPU to \"interactive\" or \"round-robin\"",
    args: &[Arg {
        from_request: true,
        aliases: &[("rr", "round-robin"), ("round robin", "round-robin")],
        ..arg("name", ArgType::Enum(POLICIES), Role::Target)
    }],
    facts: Facts::LOCAL_CHANGE,
    // The shell records the installed policy first and puts it back on `undo`.
    inverse: Inverse::Restore,
    cues: &[
        "polic",
        "schedul",
        "round robin",
        "round-robin",
        "interactive",
    ],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: Owner::Kernel,
    needs: cap::POLICY,
    min_tier: Tier::Run,
    old_name: None,
};

pub const FIXED: Action = helper(
    "fixed",
    "n / 10^places as text with exactly places decimals",
    &[
        arg("n", INT, Role::Payload),
        arg("places", ArgType::Int { min: 0, max: 9 }, Role::Option),
    ],
    RetType::Of(STRING),
);

pub const FLOOR: Action = helper(
    "floor",
    "x rounded down; numbers are integers, so x itself",
    &[arg("x", INT, Role::Payload)],
    RetType::Of(INT),
);

/// `tune [process][: intent]`: proposes a scheduler trial for a process.
pub const TUNE: Action = Action {
    name: "tune",
    doc: "proposes a scheduler trial for a process",
    args: &[
        Arg {
            from_request: true,
            never: PROTECTED,
            ..arg(
                "target",
                ArgType::Opt(&ArgType::Text { max: 24 }),
                Role::Target,
            )
        },
        arg(
            "intent",
            ArgType::Opt(&ArgType::Text { max: 120 }),
            Role::Option,
        ),
    ],
    facts: Facts {
        idempotent: false,
        ..Facts::LOCAL_CHANGE
    },
    inverse: Inverse::Compensate("tune_undo"),
    cues: &["tune"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: Owner::Kernel,
    needs: cap::POLICY,
    // The proposal card asks before the kernel starts a trial, as today.
    min_tier: Tier::Ask,
    old_name: None,
};

/// `tune undo`: puts back the policy a kept trial replaced.
pub const TUNE_UNDO: Action = Action {
    name: "tune_undo",
    doc: "puts back the policy a kept tune trial replaced",
    args: &[],
    inverse: Inverse::Restore,
    cues: &["tune"],
    min_tier: Tier::Run,
    ..TUNE
};

/// `:revert <name> [n]`: swaps a live component to an earlier version.
pub const COMPONENT_REVERT: Action = Action {
    name: "component_revert",
    doc: "swaps a live component to the nth version before",
    args: &[
        Arg {
            from_request: true,
            ..arg("component", ArgType::Text { max: 24 }, Role::Target)
        },
        // `kabi/src/live.rs` keeps six versions.
        arg(
            "back",
            ArgType::Opt(&ArgType::Int { min: 1, max: 6 }),
            Role::Option,
        ),
    ],
    facts: Facts::LOCAL_CHANGE,
    // The version it replaced stays stored, so reverting again swaps it back.
    inverse: Inverse::Restore,
    cues: &["revert", "roll back", "rollback", "previous version"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: Owner::Kernel,
    needs: cap::COMPONENT,
    min_tier: Tier::Run,
    old_name: None,
};

/// The functions LLM-written scripts may call, in the order the prompt lists them.
pub const SCRIPT_API: [Action; 7] = [
    CPUS, MEMORY, UPTIME_MS, PROCESSES, THREADS, GET_POLICY, SET_POLICY,
];
/// The pure functions every strict engine adds besides the OS functions it grants.
pub const HELPERS: [Action; 2] = [FIXED, FLOOR];
/// Rows that are typed commands today and not bound in scripts.
pub const COMMANDS: [Action; 3] = [TUNE, TUNE_UNDO, COMPONENT_REVERT];

/// Every OS row.
pub const ROWS: [Action; 12] = [
    CPUS,
    MEMORY,
    UPTIME_MS,
    PROCESSES,
    THREADS,
    GET_POLICY,
    SET_POLICY,
    FIXED,
    FLOOR,
    TUNE,
    TUNE_UNDO,
    COMPONENT_REVERT,
];
