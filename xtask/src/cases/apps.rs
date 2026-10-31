//! AP, the apps (P4): one case for each built-in app, driven by the agent through
//! the mock model, which answers a request by its text.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // P4 task 4: the AI types into the Terminal app, a person's Enter runs the line
    // (the harness's one key), and the AI reads the rows.
    TestCase {
        keys: &[&["ret"]],
        expect: &[
            "app-terminal: the AI typed a line, a person's Enter ran it, and the AI read its reply",
        ],
        ..case("app-terminal", "test=app-terminal")
    },
];
