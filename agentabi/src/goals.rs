//! The goal rows (ADR 0013 section 5; row G2 of `docs/plans/agents-os.md`): the
//! person sets, lists, pauses, resumes and cancels the standing requests the
//! `goals` agent runs. The shell carries them out (`shellcore::goalrows`): it
//! asks once for `goal_set` with the scope, budget and deadline, sets the goal's
//! ceiling (`agentid::Roster::grant_goal`) from that yes, and queues the goal for
//! the `goals` agent, which takes it with `agent-step next`.
//!
//! They are owned by the `goals` service, so the pack lists them, as one index
//! line (ADR 0010 §6): the shell intercepts them before any binding. An agent's
//! step never reaches them: goals are the person's.

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};

const OWNER: Owner = Owner::Service("goals");

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

/// The goal, by the number `goal_set` gave it.
const ID: Arg = arg(
    "id",
    ArgType::Int {
        min: 1,
        max: 1 << 40,
    },
    Role::Target,
);

const SET_ARGS: &[Arg] = &[
    arg("text", ArgType::Text { max: 200 }, Role::Payload),
    arg("steps", ArgType::Int { min: 1, max: 64 }, Role::Option),
    arg(
        "tokens",
        ArgType::Int {
            min: 1,
            max: 200_000,
        },
        Role::Option,
    ),
    // "2h", "1d", "90m": parsed by code; anything else is refused.
    arg("wall", ArgType::Text { max: 16 }, Role::Option),
    // "18:00" today, or a wall time like "2h" from now.
    arg("by", ArgType::Opt(&ArgType::Text { max: 32 }), Role::Option),
];

/// `goal_set(text, steps, tokens, wall, by?)`: a standing request. It always
/// asks, once, with what the goal may use and spend.
pub const GOAL_SET: Action = Action {
    name: "goal_set",
    doc: "sets a goal the goals agent works on; it asks first",
    args: SET_ARGS,
    facts: Facts::LOCAL_CHANGE,
    inverse: Inverse::Compensate("goal_cancel"),
    cues: &["goal", "keep working", "until done"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: OWNER,
    needs: 0,
    min_tier: Tier::Ask,
    old_name: None,
};

/// `goal_list()`: the goals set, each with its state.
pub const GOAL_LIST: Action = Action {
    name: "goal_list",
    doc: "the goals set, with their state",
    args: &[],
    facts: Facts::LOCAL_READ,
    inverse: Inverse::Read,
    cues: &["goals"],
    returns: Returns::System,
    ret: RetType::Of(ArgType::Text { max: 4096 }),
    owner: OWNER,
    needs: 0,
    min_tier: Tier::Run,
    old_name: None,
};

const fn toggle(
    name: &'static str,
    doc: &'static str,
    back: &'static str,
    cue: &'static [&'static str],
) -> Action {
    Action {
        name,
        doc,
        args: &[ID],
        facts: Facts::LOCAL_CHANGE,
        inverse: Inverse::Compensate(back),
        cues: cue,
        returns: Returns::System,
        ret: RetType::Unit,
        owner: OWNER,
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

/// `goal_pause(id)`: the goal takes no step until resumed.
pub const GOAL_PAUSE: Action = toggle("goal_pause", "pauses goal id", "goal_resume", &["pause"]);

/// `goal_resume(id)`: a paused goal goes on.
pub const GOAL_RESUME: Action = toggle("goal_resume", "resumes goal id", "goal_pause", &["resume"]);

/// `goal_cancel(id)`: the goal stops for good and its ceiling goes. Its steps
/// stay done; `undo goals` undoes them one by one.
pub const GOAL_CANCEL: Action = Action {
    name: "goal_cancel",
    doc: "cancels goal id for good; it asks first",
    args: &[ID],
    facts: Facts {
        destructive: true,
        ..Facts::LOCAL_CHANGE
    },
    inverse: Inverse::None("a cancelled goal is set again with goal_set"),
    cues: &["cancel", "stop the goal"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: OWNER,
    needs: 0,
    min_tier: Tier::Ask,
    old_name: None,
};

/// The goal rows, in the pack's index as the `goal` group.
pub const GOAL_ROWS: [Action; 5] = [GOAL_SET, GOAL_LIST, GOAL_PAUSE, GOAL_RESUME, GOAL_CANCEL];

const _: () = assert!(
    crate::check(&GOAL_ROWS).is_ok(),
    "a goal row is inconsistent"
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;

    #[test]
    fn setting_and_cancelling_ask_and_the_rest_run() {
        assert_eq!(derive(&GOAL_SET), Tier::Ask);
        assert_eq!(derive(&GOAL_CANCEL), Tier::Ask);
        assert_eq!(derive(&GOAL_LIST), Tier::Run);
        assert_eq!(derive(&GOAL_PAUSE), Tier::Run);
        assert_eq!(derive(&GOAL_RESUME), Tier::Run);
    }

    #[test]
    fn pause_and_resume_undo_each_other() {
        assert!(matches!(
            GOAL_PAUSE.inverse,
            Inverse::Compensate("goal_resume")
        ));
        assert!(matches!(
            GOAL_RESUME.inverse,
            Inverse::Compensate("goal_pause")
        ));
    }
}
