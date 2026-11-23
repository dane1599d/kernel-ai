//! The mock's answer to a quarantined read (`query`).

use serde_json::{Value, json};

/// A quarantined read's answer, as a model that obeys its data would give it:
/// a yes, the first offered word the data names (else the first), or the
/// data's first line. Whatever it says comes back to the script only as data.
pub(super) fn query(request: &Value) -> String {
    let asked = request["messages"][1]["content"].as_str().unwrap_or("");
    let data = asked
        .split_once("<<<DATA\n")
        .and_then(|(_, rest)| rest.split_once("\nDATA>>>"))
        .map_or("", |(data, _)| data);
    let answer = if asked.contains("Answer yes or no.") {
        String::from("yes")
    } else if let Some((_, words)) = asked.split_once("Answer with exactly one of: ") {
        let words: Vec<&str> = words
            .split('\n')
            .next()
            .unwrap_or("")
            .trim_end_matches('.')
            .split(", ")
            .collect();
        let named = words.iter().find(|word| data.contains(**word));
        String::from(*named.or(words.first()).unwrap_or(&""))
    } else {
        data.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .chars()
            .take(llmproto::query::MAX_SHORT_CHARS)
            .collect()
    };
    json!({ "answer": answer }).to_string()
}
