//! The OS rows of `REGISTRY_VERSION` 1: the seven M8 script functions, P1's
//! process and service rows, the shell's two pure helpers, and the typed `tune`,
//! `tune undo` and `:revert`.

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

/// A process or service named in the request; no row acts on a protected one.
const NAMED: Arg = Arg {
    from_request: true,
    never: PROTECTED,
    ..arg("name", ArgType::Text { max: 24 }, Role::Target)
};

/// Stopping loses what it held, and a stopped process cannot be resumed.
const STOPS: Facts = Facts {
    read_only: false,
    destructive: true,
    idempotent: true,
    open_world: false,
};

/// Stops a running process by name: no undo, so it asks once (ADR 0002 D2a).
pub const PROCESS_STOP: Action = Action {
    name: "process_stop",
    doc: "stops or kills processes by name",
    args: &[NAMED],
    facts: STOPS,
    inverse: Inverse::None("a stopped process cannot be resumed"),
    cues: &["stop", "kill", "end", "terminate"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: Owner::Kernel,
    needs: cap::CALL,
    min_tier: Tier::Run,
    old_name: None,
};

/// Stops a supervised service so it stays stopped: no undo, so it asks once.
pub const SERVICE_STOP: Action = Action {
    name: "service_stop",
    doc: "stays stopped until started",
    cues: &["stop", "shut", "kill"],
    inverse: Inverse::None("a stopped service loses what it held"),
    owner: Owner::Service("supervisor"),
    ..PROCESS_STOP
};

/// Starts a stopped service again; `undo` stops it.
pub const SERVICE_START: Action = Action {
    name: "service_start",
    doc: "starts a stopped service",
    facts: Facts::LOCAL_CHANGE,
    inverse: Inverse::Compensate("service_stop"),
    cues: &["start", "launch"],
    ..SERVICE_STOP
};

/// Stops a service and starts it again: what it held is lost, so it asks once.
pub const SERVICE_RESTART: Action = Action {
    name: "service_restart",
    doc: "loses what it held",
    inverse: Inverse::None("a restarted service loses what it held"),
    cues: &["restart"],
    ..SERVICE_STOP
};

/// A package named in the request (ADR 0011 §2: at most 12 bytes).
const PACKAGE: Arg = Arg {
    from_request: true,
    ..arg("name", ArgType::Text { max: 12 }, Role::Target)
};

/// `install <name>` (ADR 0011 §10): a new generation, undone by switching back.
/// The shell asks first when the package needs a new permission.
pub const PKG_INSTALL: Action = Action {
    name: "pkg_install",
    doc: "installs a package; undo puts the old set back",
    args: &[PACKAGE],
    facts: Facts::LOCAL_CHANGE,
    inverse: Inverse::Restore,
    cues: &["install", "add"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: Owner::Service("pkg"),
    needs: 0,
    min_tier: Tier::Run,
    old_name: None,
};

/// `remove <name>`: a new generation without it; its data folder stays.
pub const PKG_REMOVE: Action = Action {
    name: "pkg_remove",
    doc: "removes a package",
    cues: &["remove", "uninstall", "delete"],
    ..PKG_INSTALL
};

/// `pkg list`.
pub const PKG_LIST: Action = Action {
    owner: Owner::Service("pkg"),
    ..read("pkg_list", "installed packages", RetType::Of(STRING))
};

/// `pkg search <text>`.
pub const PKG_SEARCH: Action = Action {
    args: &[arg("text", ArgType::Text { max: 64 }, Role::Option)],
    owner: Owner::Service("pkg"),
    ..read(
        "pkg_search",
        "packages in the repository",
        RetType::Of(STRING),
    )
};

/// Rows the shell's package store answers (`shellcore::packages`).
pub const PKG_ROWS: [Action; 4] = [PKG_INSTALL, PKG_REMOVE, PKG_LIST, PKG_SEARCH];

/// A model named in the request: a signed target's id or name, or the first
/// hex digits of its SHA-256 (`comp_models::Models::find`).
const MODEL: Arg = Arg {
    from_request: true,
    ..arg("name", ArgType::Text { max: 64 }, Role::Target)
};

/// `models` (ADR 0008 §11; P4 task 10): the stored and signed models, each with
/// its licence and whether it runs, is next, or is on trial.
pub const MODEL_LIST: Action = Action {
    owner: Owner::Service("updated"),
    ..read(
        "model_list",
        "stored and signed AI models",
        RetType::Of(STRING),
    )
};

/// `model_use <name>`: a stored model the signed role takes, used from the
/// next restart on trial; `undo` keeps the model before.
pub const MODEL_USE: Action = Action {
    name: "model_use",
    doc: "uses a stored model from the next restart",
    args: &[MODEL],
    facts: Facts::LOCAL_CHANGE,
    inverse: Inverse::Restore,
    cues: &["model", "switch"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: Owner::Service("updated"),
    needs: 0,
    min_tier: Tier::Run,
    old_name: None,
};

/// `model_fetch disk`: the attached disk's model copied into the store, if the
/// signed role names it by length and SHA-256. It asks once: it brings
/// gigabytes of new bytes. The one source is the import disk until P5's
/// downloads.
pub const MODEL_FETCH: Action = Action {
    name: "model_fetch",
    doc: "imports a signed model from the attached disk",
    args: &[arg("source", ArgType::Enum(&["disk"]), Role::Option)],
    // No undo yet: an imported model stays until a later row removes it, so
    // it is marked destructive, and asks, as every row with no undo does.
    facts: Facts {
        destructive: true,
        ..Facts::LOCAL_CHANGE
    },
    inverse: Inverse::None("an imported model stays in the store"),
    cues: &["import", "fetch", "download"],
    min_tier: Tier::Ask,
    ..MODEL_USE
};

/// Rows `updated` answers through the shell (`shellcore::models`).
pub const MODEL_ROWS: [Action; 3] = [MODEL_LIST, MODEL_USE, MODEL_FETCH];

/// The functions LLM-written scripts may call, in the order the prompt lists them.
pub const SCRIPT_API: [Action; 7] = [
    CPUS, MEMORY, UPTIME_MS, PROCESSES, THREADS, GET_POLICY, SET_POLICY,
];
/// Rows a tagged call runs but a script may not: what they change asks once, so
/// a script's one consent could not cover them. The script prompt does not list
/// them; they reach the model only through the act pack.
pub const CALL_ONLY: [Action; 5] = [
    PROCESS_STOP,
    SERVICE_STOP,
    SERVICE_START,
    SERVICE_RESTART,
    crate::APPEARANCE_SET,
];
/// Every row the shell's script engine binds: the script API, then the call-only
/// rows, which only a tagged call's one-line script is given.
pub const BOUND: [Action; 12] = [
    CPUS,
    MEMORY,
    UPTIME_MS,
    PROCESSES,
    THREADS,
    GET_POLICY,
    SET_POLICY,
    PROCESS_STOP,
    SERVICE_STOP,
    SERVICE_START,
    SERVICE_RESTART,
    crate::APPEARANCE_SET,
];
/// The pure functions every strict engine adds besides the OS functions it grants.
pub const HELPERS: [Action; 2] = [FIXED, FLOOR];
/// Rows that are typed commands today and not bound in scripts.
pub const COMMANDS: [Action; 7] = [
    TUNE,
    TUNE_UNDO,
    COMPONENT_REVERT,
    PKG_INSTALL,
    PKG_REMOVE,
    PKG_LIST,
    PKG_SEARCH,
];

/// `rules`: lists the standing rules the menu's "Always allow" wrote.
pub const RULES: Action = Action {
    name: "rules",
    doc: "the standing rules, numbered",
    cues: &["rules"],
    owner: Owner::Shell,
    ..read("rules", "", RetType::Of(STRING))
};

/// `rules_remove <n>`: removes one rule and revokes its grant node. It only
/// narrows what runs without a question.
pub const RULES_REMOVE: Action = Action {
    name: "rules_remove",
    doc: "removes standing rule n",
    args: &[arg("n", ArgType::Int { min: 1, max: 32 }, Role::Target)],
    facts: Facts::LOCAL_CHANGE,
    inverse: Inverse::Restore,
    cues: &["remove", "delete"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: Owner::Shell,
    needs: 0,
    min_tier: Tier::Run,
    old_name: None,
};

/// Rows only the user types (ADR 0002 D2a): standing consent is the user's alone,
/// so these are in no pack and no tagged call or script can reach them. They are
/// not in [`ROWS`], which is what calls, consent and the pack look in.
pub const TYPED_ONLY: [Action; 2] = [RULES, RULES_REMOVE];

/// Every OS row.
pub const ROWS: [Action; 42] = [
    CPUS,
    MEMORY,
    UPTIME_MS,
    PROCESSES,
    THREADS,
    GET_POLICY,
    SET_POLICY,
    PROCESS_STOP,
    SERVICE_STOP,
    SERVICE_START,
    SERVICE_RESTART,
    FIXED,
    FLOOR,
    TUNE,
    TUNE_UNDO,
    COMPONENT_REVERT,
    PKG_INSTALL,
    PKG_REMOVE,
    PKG_LIST,
    PKG_SEARCH,
    crate::FS_LIST,
    crate::FS_READ,
    crate::FS_HISTORY,
    crate::FS_CHANGES,
    crate::FS_CREATE,
    crate::FS_WRITE,
    crate::FS_MOVE,
    crate::FS_TRASH,
    crate::FS_RESTORE,
    crate::FS_EMPTY_TRASH,
    crate::UI_WINDOWS,
    crate::UI_VIEW,
    crate::UI_OPEN,
    crate::UI_FOCUS,
    crate::WINDOW_CLOSE,
    crate::UI_ACT,
    crate::APPEARANCE_SET,
    crate::GRANT_REVOKE,
    crate::GRANT_RESTORE,
    MODEL_LIST,
    MODEL_USE,
    MODEL_FETCH,
];
