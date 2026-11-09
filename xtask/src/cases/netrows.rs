//! P5 task 11, the agent's network rows (lane A-core): consent for what the
//! agent sends out.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // P-F (ADR 0002 D6): read content goes out only to a host the line names.
    // A note's text, and a page's, each sent elsewhere by the mock model, ask
    // and name their source; declined, nothing is sent. Consent decides on the
    // plan, so no network is needed.
    TestCase {
        expect: &[
            "net-pf: a note's text and a page's, sent to a host the line did not name, each asked and named its source; declined, nothing was sent",
        ],
        ..case("net-pf", "test=net-pf")
    },
];
