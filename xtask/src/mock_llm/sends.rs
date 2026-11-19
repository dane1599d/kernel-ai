//! The mock model's send for `desk-sends` (ADR 0005 §7, "Amended
//! 2026-11-19"): the line typed in the Kai pane is answered with a
//! `net_send` whose text is longer than the pane shows at 1920x1080, so the
//! case must scroll it through before the card takes an Allow. Apart from
//! `mock_llm.rs`, which is long already; its `act` asks here first.

use serde_json::json;

/// The line `desk-sends` types (`kernel/src/desk_sends_tests.rs`).
const LINE: &str = "send my long note to hooks.example";
/// The text's lines; the kernel case makes the same text for its digest.
const TEXT_LINES: usize = 100;

/// The text the send carries: numbered lines, as the kernel case's `text()`.
fn text() -> String {
    let lines: Vec<String> = (1..=TEXT_LINES)
        .map(|n| format!("{n:03} the text goes on"))
        .collect();
    lines.join("\n")
}

/// The call for an act request's `line`, when it is the case's.
pub(super) fn call(line: &str) -> Option<String> {
    let arguments = json!({"host": "hooks.example", "path": "/hook", "text": text()});
    (line == LINE).then(|| json!({"name": "net_send", "arguments": arguments}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cases_line_gets_a_send_call_and_others_get_none() {
        let call = call(LINE).expect("the case's line");
        assert!(llmproto::act::parse(&call).is_ok(), "{call}");
        let value: serde_json::Value = serde_json::from_str(&call).unwrap();
        assert_eq!(value["arguments"]["text"], text());
        assert_eq!(super::call("send hello to hooks.example"), None);
    }

    #[test]
    fn the_act_route_answers_the_line_with_the_send() {
        let request = json!({"messages": [
            {"role": "system", "content": "x"},
            {"role": "user", "content": llmproto::pack::MODE_ACT},
            {"role": "user", "content": format!("Request: {LINE}")},
        ]});
        assert_eq!(super::super::act(&request), call(LINE).unwrap());
    }

    /// The kernel case types the same line and digests the same text, whose
    /// length its expected pane line names.
    #[test]
    fn the_kernel_case_makes_the_same_line_and_text() {
        let kernel = include_str!("../../../kernel/src/desk_sends_tests.rs");
        assert!(kernel.contains(&format!("LINE: &str = \"{LINE}\"")));
        assert!(kernel.contains(&format!("TEXT_LINES: usize = {TEXT_LINES};")));
        assert!(kernel.contains("format!(\"{n:03} the text goes on\")"));
        assert_eq!(text().len(), 2099);
    }
}
