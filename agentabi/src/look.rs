//! The appearance row (ADR 0005 §8, P4 task 8): one setting of the user's
//! appearance, run at once when asked and undone by putting the old value
//! back. The settings and their values are `look::appearance`'s; the shell
//! checks every value there (a theme that exists, contrast kept).

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};

/// `look::appearance::SETTINGS`, which a shellcore test keeps equal.
pub const APPEARANCE_SETTINGS: &[&str] = &[
    "theme",
    "token",
    "scale",
    "text_size",
    "reduced_motion",
    "legible",
];

/// Sets one appearance setting; `undo` puts its old value back.
pub const APPEARANCE_SET: Action = Action {
    name: "appearance_set",
    doc: "sets an appearance setting: theme, text size, motion and so on",
    args: &[
        Arg {
            name: "setting",
            ty: ArgType::Enum(APPEARANCE_SETTINGS),
            role: Role::Target,
            from_request: true,
            never: &[],
            aliases: &[
                ("text size", "text_size"),
                ("reduced motion", "reduced_motion"),
                ("motion", "reduced_motion"),
                ("legible type", "legible"),
                ("colour", "token"),
                ("color", "token"),
            ],
        },
        Arg {
            name: "value",
            ty: ArgType::Text { max: 24 },
            role: Role::Option,
            from_request: true,
            never: &[],
            aliases: &[],
        },
    ],
    facts: Facts::LOCAL_CHANGE,
    // The shell keeps the value it replaced and puts it back on `undo`.
    inverse: Inverse::Restore,
    cues: &[
        "theme",
        "appearance",
        "dark",
        "light",
        "text size",
        "bigger",
        "smaller",
        "motion",
        "legible",
        "colour",
        "color",
    ],
    returns: Returns::System,
    ret: RetType::Unit,
    // `desktop` draws with it (ADR 0005 §8); the shell keeps and saves it.
    owner: Owner::Service("desktop"),
    needs: 0,
    min_tier: Tier::Run,
    old_name: None,
};
