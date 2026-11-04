//! The agent's network rows (P5 task 11, ADR 0002 D3): reads that leave the
//! machine. Each is open-world, so a call runs only when its host is grounded
//! (`nethosts::Grounding`, through `consent`): typed in the line, a host-table
//! word of it, granted before, or the OS's own checked source; any other asks.
//! A word in a clause that says not to ("don't fetch weather") grounds
//! nothing. What a fetch reads is `Data`, so P-F holds for what follows:
//! read content may go out only to a host the line names. `:resolve` and
//! `:ping` are read rows too, since a resolve and a ping reach the network.

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
        owner: Owner::Service("netd"),
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

pub const NET_RESOLVE: Action = Action {
    cues: &["resolve", "dns", "address"],
    ..read(
        "net_resolve",
        "the addresses a host name resolves to",
        &[HOST],
        Returns::Content("dns"),
        RetType::Data,
    )
};

pub const NET_PING: Action = Action {
    cues: &["ping", "reachable", "latency"],
    ..read(
        "net_ping",
        "whether a host answers, and in how many ms",
        &[HOST],
        Returns::System,
        RetType::Of(ArgType::Int {
            min: -1,
            max: 60_000,
        }),
    )
};

/// The network rows, in `ROWS` order.
pub const NET_ROWS: [Action; 3] = [NET_FETCH, NET_RESOLVE, NET_PING];
