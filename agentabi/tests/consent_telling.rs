//! A telling word ("list", "show", "describe") before a cue keeps its clause a
//! telling one: the change is not asked for. The one exception is a telling
//! word used as a noun right before the cue word, after a determiner or a noun:
//! "build me a shopping list app" asks for the app (#1202's boot case). Every
//! line here is pinned against a real change row whose arguments the line
//! names, beside the plain order that does ask, so only the telling rule can
//! decide it (the deep review of #1214).

use agentabi::ROWS;
use agentabi::consent::{Call, Label, Value, Why, decide};

const fn lit(text: &'static str) -> Value<'static> {
    Value {
        text,
        label: Label::Literal,
    }
}

const APP_BUILD: Call<'static> = Call {
    name: "app_build",
    args: &[
        ("name", lit("shopping")),
        ("request", lit("a shopping list")),
    ],
    control: Label::Literal,
};
const TRASH_IDEAS: Call<'static> = Call {
    name: "fs_trash",
    args: &[("path", lit("/home/user/ideas"))],
    control: Label::Literal,
};
const INSTALL_HTOP: Call<'static> = Call {
    name: "pkg_install",
    args: &[("name", lit("htop"))],
    control: Label::Literal,
};
const ROUND_ROBIN: Call<'static> = Call {
    name: "set_policy",
    args: &[("name", lit("round-robin"))],
    control: Label::Literal,
};

fn why(line: &str, call: Call<'static>) -> Why {
    decide(line, &[call], &ROWS)[0].why
}

/// The plain orders: each row's call is asked for, so a "not asked" below is
/// the telling rule's doing, not a missing argument.
#[test]
fn the_plain_orders_ask_for_their_change() {
    for (line, call) in [
        ("delete ideas", TRASH_IDEAS),
        ("install htop", INSTALL_HTOP),
        ("switch to round robin", ROUND_ROBIN),
        ("build me a shopping list app", APP_BUILD),
    ] {
        assert_eq!(why(line, call), Why::Asked, "{line}");
    }
}

/// A telling word used as a noun right before the cue names the thing asked
/// for, and asks to be told nothing.
#[test]
fn a_telling_noun_right_before_the_cue_leaves_the_line_asking() {
    for line in [
        "build me a shopping list app",
        "make a grocery list app",
        "write me a weekly report app",
        "build my list app",
    ] {
        assert_eq!(why(line, APP_BUILD), Why::Asked, "{line}");
    }
}

/// Every other telling word before the cue keeps the line a telling one, with
/// any filler before it: the change is never taken as asked for.
#[test]
fn any_other_telling_word_before_the_cue_asks_for_nothing() {
    for (line, call) in [
        ("can you briefly explain how to remove ideas", TRASH_IDEAS),
        ("I'd like you to explain how to delete ideas", TRASH_IDEAS),
        ("pls describe how to delete ideas", TRASH_IDEAS),
        ("tell me how to delete ideas", TRASH_IDEAS),
        (
            "give me a list of files I could delete, like ideas",
            TRASH_IDEAS,
        ),
        (
            "what's the list of packages to install, like htop",
            INSTALL_HTOP,
        ),
        (
            "show me a report of what to install, like htop",
            INSTALL_HTOP,
        ),
        ("explain how to switch to round robin", ROUND_ROBIN),
        ("can you briefly describe round robin", ROUND_ROBIN),
        ("tell me how to build an app", APP_BUILD),
        (
            "can you explain how to build a shopping list app",
            APP_BUILD,
        ),
        ("I want you to show me how to build an app", APP_BUILD),
        // Fails safe: "to-do" splits into "to" and "do", and "do" is no noun,
        // so this one still asks before it builds.
        ("make a to-do list app", APP_BUILD),
    ] {
        assert_ne!(why(line, call), Why::Asked, "{line}");
    }
}

/// A wh-contraction leads a question as its full form does.
#[test]
fn a_wh_contraction_asks_to_be_told() {
    for line in [
        "what's round robin",
        "how's the round robin policy different",
    ] {
        assert_ne!(why(line, ROUND_ROBIN), Why::Asked, "{line}");
    }
}
