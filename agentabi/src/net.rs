//! The agent's network rows (P5 task 11, ADR 0002 D3): reads that leave the
//! machine. Each is open-world, so a call runs only when its host is grounded
//! (`nethosts::Grounding`, through `consent`): typed in the line, a host-table
//! word of it, granted before, or the OS's own checked source; any other asks.
//! A word in a clause that says not to ("don't fetch weather") grounds
//! nothing. What a fetch reads is `Data`, so P-F holds for what follows:
//! read content may go out only to a host the line names. `:resolve` and
//! `:ping` are one read row, `net_lookup`, since both reach the network.

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};

/// A read that leaves the machine.
const OPEN_READ: Facts = Facts {
    open_world: true,
    ..Facts::LOCAL_READ
};

/// The host a call reaches: it must be grounded for the call to run.
const HOST: Arg = Arg {
    name: "host",
    ty: ArgType::Host,
    role: Role::Target,
    from_request: true,
    never: &[],
    aliases: &[],
};

/// The path and query on the host, if any: how, not where.
const PATH: Arg = Arg {
    name: "path",
    ty: ArgType::Opt(&ArgType::Text { max: 255 }),
    role: Role::Option,
    from_request: false,
    never: &[],
    aliases: &[],
};

const fn read(
    name: &'static str,
    doc: &'static str,
    args: &'static [Arg],
    returns: Returns,
    ret: RetType,
) -> Action {
    Action {
        name,
        doc,
        args,
        facts: OPEN_READ,
        inverse: Inverse::Read,
        cues: &[],
        returns,
        ret,
        owner: Owner::Service("fetch"),
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

pub const NET_FETCH: Action = Action {
    cues: &["fetch", "download", "look up", "web", "page", "site"],
    ..read(
        "net_fetch",
        "a web page or file from a host; its text is data",
        &[HOST, PATH],
        Returns::Content("web"),
        RetType::Data,
    )
};

/// `:resolve` and `:ping` as one read row, to keep the pack in its budget
/// (Token counts run 41881793165: three rows put it 9 tokens over).
pub const NET_LOOKUP: Action = Action {
    cues: &["resolve", "dns", "address", "ping", "reachable", "latency"],
    ..read(
        "net_lookup",
        "a host's addresses, and how many ms it takes to answer",
        &[HOST],
        Returns::Content("dns"),
        RetType::Data,
    )
};

/// What was read, which the question is about: data, never instructions.
const DATA: Arg = Arg {
    name: "data",
    ty: ArgType::Text { max: 4096 },
    role: Role::Payload,
    from_request: false,
    never: &[],
    aliases: &[],
};

/// The one question asked about it.
const QUESTION: Arg = Arg {
    name: "question",
    ty: ArgType::Text { max: 200 },
    role: Role::Option,
    from_request: false,
    never: &[],
    aliases: &[],
};

/// The answers allowed: none for one short line, or the words to pick from
/// (`["yes", "no"]` for a yes or no).
const ANSWERS: Arg = Arg {
    name: "answers",
    ty: ArgType::Opt(&ArgType::List(&ArgType::Text { max: 32 }, 8)),
    role: Role::Option,
    from_request: false,
    never: &[],
    aliases: &[],
};

/// A quarantined read (ADR 0002 D6, FIDES's typed query): one question about
/// read data, asked of the model apart, with no tools, no pack and no history,
/// its answer held to `answers` or one short line. The answer is `Data` with
/// the data's label, so whatever the data said can pick at most an allowed
/// answer, never a call. It stays on the machine: `llm` answers it.
pub const QUERY: Action = Action {
    name: "query",
    doc: "one answer about read data: one of answers, or one short line",
    args: &[DATA, QUESTION, ANSWERS],
    facts: Facts::LOCAL_READ,
    inverse: Inverse::Read,
    cues: &[],
    returns: Returns::Content("query"),
    ret: RetType::Data,
    owner: Owner::Service("llm"),
    needs: 0,
    min_tier: Tier::Run,
    old_name: None,
};

/// P5 task 11's rows, in `ROWS` order: the network reads, then the question
/// asked about what they read.
pub const NET_ROWS: [Action; 3] = [NET_FETCH, NET_LOOKUP, QUERY];
