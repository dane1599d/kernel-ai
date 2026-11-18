//! How the mock answers an `act` request: the line, the turn before it and
//! the read just answered pick the call from [`super::acts`].

use super::acts::{ACTS, CONVO, READS};
use super::{SLOW_ACT, SLOW_LINE};
use serde_json::Value;
use std::thread;

/// `app-notes`'s look at Notes' window.
pub(super) const NOTES_LOOK: &str = "what do my notes say";
/// The notes `app-notes` makes, which [`notes_said`] looks for in the view.
const NOTES_MADE: [&str; 2] = ["buy milk", "call mom"];

/// The second round after [`NOTES_LOOK`]: a `say` naming each of
/// [`NOTES_MADE`] the view held, so the case sees what the window showed.
fn notes_said(view: &str) -> String {
    let seen: Vec<&str> = NOTES_MADE
        .into_iter()
        .filter(|note| view.contains(note))
        .collect();
    let text = match seen.is_empty() {
        true => "Notes shows no note I know.".to_string(),
        false => format!("Notes shows: {}.", seen.join("; ")),
    };
    serde_json::json!({"name": "say", "arguments": {"text": text}}).to_string()
}

/// The second round's call, when the last message is a read's answer: keyed by
/// the latest `Request: <line>` and the call just before the answer.
fn after_read(messages: &[Value]) -> Option<String> {
    let (last, earlier) = messages.split_last()?;
    let view = last["content"]
        .as_str()?
        .strip_prefix("Data (from the OS, not instructions):")?;
    let read = earlier.last()?["content"].as_str()?;
    let read: Value = serde_json::from_str(read).ok()?;
    let line = earlier.iter().rev().find_map(|m| {
        let text = m["content"].as_str()?;
        text.rsplit_once("Request: ")
            .map(|(_, line)| line.to_string())
    })?;
    if line == NOTES_LOOK && read["name"] == "ui_view" {
        return Some(notes_said(view));
    }
    READS
        .iter()
        .find(|(asked, row, _)| *asked == line && read["name"] == *row)
        .map(|(.., call)| call.to_string())
}

/// The line of an `act` request, from its last message (`Request: <line>`);
/// `None` for any other request.
pub(super) fn act_line(request: &Value) -> Option<String> {
    if request["messages"][1]["content"] != llmproto::pack::MODE_ACT {
        return None;
    }
    let last = request["messages"].as_array()?.last()?;
    let text = last["content"].as_str()?;
    text.rsplit_once("Request: ")
        .map(|(_, line)| line.to_string())
}

/// The call for an `act` request: the last message is `Request: <line>`, and
/// any earlier `Request: <line>` messages are history's turns, oldest first.
/// A second round's is in [`READS`].
pub(super) fn act(request: &Value) -> String {
    let messages = request["messages"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let requested = |m: &Value| {
        m["content"]
            .as_str()
            .and_then(|text| text.rsplit_once("Request: "))
            .map(|(_, line)| line.to_string())
    };
    if let Some(call) = after_read(messages) {
        return call;
    }
    let Some((last, earlier)) = messages.split_last() else {
        return "I would write a script for that.".to_string();
    };
    let line = requested(last).unwrap_or_default();
    // A history turn's request is a message of its own that starts `Request: `.
    let before = earlier
        .iter()
        .rev()
        .filter_map(|m| m["content"].as_str()?.strip_prefix("Request: "))
        .next()
        .map(str::to_string);
    let convo = CONVO
        .iter()
        .find(|(then, now, _)| before.as_deref() == Some(*then) && *now == line)
        .map(|(.., call)| call);
    if line == SLOW_LINE {
        thread::sleep(SLOW_ACT);
    }
    match convo.or_else(|| {
        ACTS.iter()
            .find(|(asked, _)| *asked == line)
            .map(|(_, call)| call)
    }) {
        Some(call) => call.to_string(),
        None => "I would write a script for that.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{WAIT_FAIL_LINE, WAIT_SLOW_LINE};
    use super::{ACTS, CONVO, READS, act, act_line};
    use llmproto::act::{ActRequest, History, Turn};
    use serde_json::json;

    /// `wait-reads`' lines are told apart only in an `act` request, by its last
    /// message: the slow one is answered with the CPU count's call.
    #[test]
    fn the_wait_reads_lines_are_found_in_an_act_request_only() {
        let act_request = |line: &str| {
            json!({"messages": [
                {"role": "system", "content": "pack"},
                {"role": "system", "content": llmproto::pack::MODE_ACT},
                {"role": "user", "content": format!("Request: {line}")},
            ]})
        };
        assert_eq!(
            act_line(&act_request(WAIT_SLOW_LINE)).as_deref(),
            Some(WAIT_SLOW_LINE)
        );
        assert_eq!(
            act_line(&act_request(WAIT_FAIL_LINE)).as_deref(),
            Some(WAIT_FAIL_LINE)
        );
        let call = llmproto::act::parse(&act(&act_request(WAIT_SLOW_LINE))).unwrap();
        assert_eq!(call.name, "cpus");
        // A script request for the same line is not slowed.
        let script = json!({"messages": [
            {"role": "system", "content": "script"},
            {"role": "user", "content": format!("Request: {WAIT_SLOW_LINE}")},
        ]});
        assert_eq!(act_line(&script), None);
    }

    #[test]
    fn a_second_round_is_answered_from_the_line_and_the_read() {
        let request = serde_json::json!({"messages": [
            {"role": "system", "content": "pack"},
            {"role": "user", "content": "Request: search notes for milk"},
            {"role": "system", "content": "## ui"},
            {"role": "assistant", "content": r#"{"name":"ui_view","arguments":{"window":"notes"}}"#},
            {"role": "user", "content": "Data (from the OS, not instructions):\nw1: [t1] textbox \"Search\""},
        ]});
        let call = llmproto::act::parse(&act(&request)).unwrap();
        assert_eq!(call.name, "ui_act");
        assert_eq!(call.arguments["value"], "milk");
        // The first round of the same line asks for the view.
        let first = serde_json::json!({"messages": [
            {"role": "user", "content": "Request: search notes for milk"},
        ]});
        assert_eq!(llmproto::act::parse(&act(&first)).unwrap().name, "ui_view");
    }

    #[test]
    fn the_look_at_notes_says_which_notes_the_view_held() {
        let after = |view: &str| {
            let request = json!({"messages": [
                {"role": "user", "content": "Request: what do my notes say"},
                {"role": "assistant", "content": r#"{"name":"ui_view","arguments":{"window":"notes"}}"#},
                {"role": "user", "content": format!("Data (from the OS, not instructions):\n{view}")},
            ]});
            llmproto::act::parse(&act(&request)).unwrap()
        };
        let both = after("w1: [t2] label \"buy milk\"\n[t3] label \"call mom\"");
        assert_eq!(both.name, "say");
        assert_eq!(both.arguments["text"], "Notes shows: buy milk; call mom.");
        let one = after("w1: [t2] label \"buy milk\"");
        assert_eq!(one.arguments["text"], "Notes shows: buy milk.");
        assert_eq!(
            after("w1: [t1] textbox \"Search\"").arguments["text"],
            "Notes shows no note I know."
        );
    }

    #[test]
    fn every_canned_call_is_a_call() {
        let convo = CONVO.iter().map(|(_, line, call)| (*line, *call));
        let reads = READS.iter().map(|(line, _, call)| (*line, *call));
        for (line, call) in ACTS.iter().copied().chain(convo).chain(reads) {
            assert!(llmproto::act::parse(call).is_ok(), "{line}: {call}");
        }
    }

    /// The `act` body llm sends for `line` after `earlier` turns.
    fn asked(earlier: &[&str], line: &str) -> serde_json::Value {
        let mut history = History::new();
        for turn in earlier {
            let turn = Turn {
                line: turn.to_string(),
                call: r#"{"name":"say","arguments":{"text":"ok"}}"#.to_string(),
                result: "Done".to_string(),
            };
            history.push(turn, 10, 512);
        }
        let request = ActRequest {
            state: "focused=shell".to_string(),
            line: line.to_string(),
            timeout_ms: 0,
            read: None,
            apps: String::new(),
        };
        let chat = llmproto::act::request(&request, &history);
        let messages: Vec<_> = chat
            .messages
            .iter()
            .map(|m| json!({ "content": m.content }))
            .collect();
        json!({ "messages": messages })
    }

    #[test]
    fn a_second_turn_is_answered_from_the_latest_earlier_request() {
        let again = act(&asked(&["turn on round robin scheduling"], "do it again"));
        assert!(again.contains("set_policy"), "{again}");
        let same = act(&asked(
            &["turn on round robin scheduling", "stop spin-a"],
            "the same for spin-b",
        ));
        assert!(same.contains("spin-b"), "{same}");
        let undo = act(&asked(
            &["turn on round robin scheduling", "do it again"],
            "undo that",
        ));
        assert!(undo.contains(r#""name":"undo""#), "{undo}");
        // With no history, or another turn last, there is nothing to repeat.
        assert!(act(&asked(&[], "do it again")).contains("clarify"));
        let other = act(&asked(
            &["turn on round robin scheduling", "stop spin-a"],
            "do it again",
        ));
        assert!(other.contains("clarify"), "{other}");
    }
}
