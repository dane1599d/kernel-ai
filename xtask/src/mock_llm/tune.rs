//! The mock's answer to a `tune` request.

use serde_json::{Value, json};

/// The canned answer to a `tune` request, keyed by the founder's intent in the
/// summary: `good pick` labels the build jobs under classes/firm, `slow pick`
/// the same under classes/gentle, `round robin` asks for round-robin, `invalid
/// answer` is prose; anything else keeps.
pub(super) fn tune(request: &Value) -> String {
    let summary = request["messages"][1]["content"].as_str().unwrap_or("");
    let schema = &request["response_format"]["json_schema"]["schema"];
    let builds: Vec<&str> = schema["properties"]["background"]["items"]["enum"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| name.starts_with("build-"))
        .collect();
    let answer = |action: &str, policy: &str, preset: &str, background: &[&str]| {
        json!({
            "reason": "The build jobs wake often and delay the editor.",
            "action": action,
            "policy": policy,
            "preset": preset,
            "background": background,
        })
        .to_string()
    };
    if summary.contains("good pick") {
        answer("try", "classes", "firm", &builds)
    } else if summary.contains("slow pick") {
        answer("try", "classes", "gentle", &builds)
    } else if summary.contains("round robin") {
        answer("try", "round-robin", "gentle", &[])
    } else if summary.contains("invalid answer") {
        String::from("Sure! I would use classes for the builds.")
    } else {
        answer("keep", "interactive", "gentle", &[])
    }
}
