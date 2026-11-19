//! A telling word ("list", "show", "describe") makes a clause ask to be told
//! only when it is used as one: leading its clause, after nothing but polite or
//! modal words. As a noun it tells nothing, so "build me a shopping list app"
//! asks for the app (#1202's app-build boot case).

use agentabi::ROWS;
use agentabi::consent::{Call, Label, Value, Why, decide};

const APP_BUILD: Call<'static> = Call {
    name: "app_build",
    args: &[
        (
            "name",
            Value {
                text: "shopping",
                label: Label::Literal,
            },
        ),
        (
            "request",
            Value {
                text: "a shopping list",
                label: Label::Literal,
            },
        ),
    ],
    control: Label::Literal,
};

fn why(line: &str) -> Why {
    decide(line, &[APP_BUILD], &ROWS)[0].why
}

#[test]
fn a_telling_word_used_as_a_noun_leaves_the_line_asking() {
    for line in [
        "build me a shopping list app",
        "make a to-do list app",
        "write me a weekly report app",
        "build a slide show app for my photos",
    ] {
        assert_eq!(why(line), Why::Asked, "{line}");
    }
}

#[test]
fn a_telling_word_used_as_a_verb_still_asks_to_be_told() {
    for line in [
        "tell me how to build an app",
        "can you explain how to build an app",
        "please describe how to make an app",
        "I want you to show me how to build an app",
    ] {
        assert_ne!(why(line), Why::Asked, "{line}");
    }
}
