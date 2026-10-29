//! T, the terminal compiler (ADR 0012): the `c>` prompt through the shell and
//! the `cc` service.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // P4 task 12's exit case: cc not running until the first entry starts it
    // (`start=demand`), then `:c`, definitions, a call, a redefinition that
    // switches its caller, `:dis`, an entry left open, a `kai` call through the
    // one door and its `undo`, then `:rhai`. Then S12-1's compile time,
    // reported, over a 200-line prelude.
    TestCase {
        expect: &[
            "c-prompt: cc was not running until the first entry started it",
            "c-prompt: :c, definitions, a redefinition that switched its caller, :dis, a kai call and its undo, :rhai",
        ],
        ..case("c-prompt", "test=c-prompt")
    },
];
