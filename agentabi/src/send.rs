//! The first send row (P5 close, ADR 0002 D2's `Hold { ms }`): `net_send`,
//! an HTTPS POST of a short text to a site the user's line names.
//!
//! - It always asks (`derive` gives a `Hold` row Ask while ADR 0002
//!   question 3 says so), and its card offers Allow and Deny only.
//! - After Allow it waits [`HOLD_MS`] in the shell's outbox with `undo` live,
//!   then goes once: never retried, never redirected, never after a restart.
//! - Its host must be one the line names (typed, or a word of the host
//!   table): never a host only an app's grant grounds, nor one read content
//!   gave (`consent`'s send-host rule and P-T).
//! - A tagged call only: it is in no script's row set ([`SEND_ROWS`] is apart
//!   from `NET_ROWS`, which `SCRIPT_CALLS` takes), and a `c>` entry may not
//!   make one.

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};

/// How long an allowed send waits for `undo` before it goes.
pub const HOLD_MS: u32 = 5_000;
/// The longest text a send carries.
pub const SEND_MAX: u16 = 4096;

/// The host the text goes to: one the line names.
const TO: Arg = Arg {
    name: "host",
    ty: ArgType::Host,
    role: Role::Target,
    from_request: true,
    never: &[],
    aliases: &[],
};

/// The path on the host, if any.
const PATH: Arg = Arg {
    name: "path",
    ty: ArgType::Opt(&ArgType::Text { max: 255 }),
    role: Role::Option,
    from_request: false,
    never: &[],
    aliases: &[],
};

/// What is sent.
const TEXT: Arg = Arg {
    name: "text",
    ty: ArgType::Text { max: SEND_MAX },
    role: Role::Payload,
    from_request: false,
    never: &[],
    aliases: &[],
};

/// A change that leaves the machine and is not taken back once it goes.
const SEND: Facts = Facts {
    read_only: false,
    destructive: false,
    idempotent: false,
    open_world: true,
};

pub const NET_SEND: Action = Action {
    name: "net_send",
    doc: "post text to a site the user names",
    args: &[TO, PATH, TEXT],
    facts: SEND,
    inverse: Inverse::Hold { ms: HOLD_MS },
    cues: &["send", "post", "submit"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: Owner::Service("fetch"),
    needs: 0,
    min_tier: Tier::Ask,
    old_name: None,
};

/// The send rows: tagged calls only, in no script's row set.
pub const SEND_ROWS: [Action; 1] = [NET_SEND];
