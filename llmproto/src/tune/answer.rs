//! The answer side of `tune`: the schema the model answers in, and the strict
//! check that turns its answer into a decision. Anything invalid is inert.

use super::{MAX_BACKGROUND, REASON_MAX_CHARS, Summary};
use crate::chat::Completion;
use crate::text;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sched::spec::{PolicyKind, PolicySpec, Preset};
use serde::Deserialize;

/// The answer's fields in the order the model must write them: the reason first,
/// so it is written before the choice it explains.
pub const FIELD_ORDER: [&str; 5] = ["reason", "action", "policy", "preset", "background"];

/// The answer's JSON Schema as JSON text, with `background` an enum of the
/// summary's labelable names: for L5's constrained decoder, and inside OpenAI's
/// `response_format`. It is text, not a `serde_json::Value`, because a value's
/// map sorts its keys, and llama.cpp's grammar and OpenAI's structured outputs
/// make the model write the fields in the order `properties` lists them: here
/// [`FIELD_ORDER`], the reason first.
pub fn schema(labelable: &[String]) -> String {
    let list = |items: &mut dyn Iterator<Item = &str>| -> String {
        let quoted: Vec<String> = items.map(|item| format!("\"{item}\"")).collect();
        quoted.join(",")
    };
    // Names are sanitized to [a-z0-9-]; anything else would need escaping, so it
    // is left out rather than trusted.
    let safe = |name: &&String| {
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    let names = list(&mut labelable.iter().filter(safe).map(String::as_str));
    let background = match names.is_empty() {
        true => String::from(r#"{"type":"array","maxItems":0}"#),
        false => format!(
            r#"{{"type":"array","maxItems":{MAX_BACKGROUND},"uniqueItems":true,"items":{{"type":"string","enum":[{names}]}}}}"#
        ),
    };
    let policies = list(&mut PolicyKind::ALL.iter().map(|kind| kind.name()));
    let presets = list(&mut Preset::ALL.iter().map(|preset| preset.name()));
    let order = list(&mut FIELD_ORDER.iter().copied());
    format!(
        r#"{{"type":"object","properties":{{"reason":{{"type":"string","maxLength":{REASON_MAX_CHARS}}},"action":{{"type":"string","enum":["keep","try"]}},"policy":{{"type":"string","enum":[{policies}]}},"preset":{{"type":"string","enum":[{presets}]}},"background":{background}}},"required":[{order}],"additionalProperties":false}}"#
    )
}

/// OpenAI's `response_format` around [`schema`], as JSON text.
pub fn response_format(labelable: &[String]) -> String {
    format!(
        r#"{{"type":"json_schema","json_schema":{{"name":"sched_tune","strict":true,"schema":{}}}}}"#,
        schema(labelable)
    )
}

/// What happens after the model answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing changes. `invalid` says why the answer was refused, in plain
    /// words, when the model did not choose keep itself.
    Keep {
        reason: String,
        invalid: Option<String>,
    },
    /// A trial of `spec` with these processes labelled background.
    Try {
        reason: String,
        spec: PolicySpec,
        background: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    reason: String,
    action: String,
    policy: String,
    preset: String,
    background: Vec<String>,
}

fn refuse(why: impl Into<String>) -> Decision {
    Decision::Keep {
        reason: String::new(),
        invalid: Some(why.into()),
    }
}

fn check_labels(names: &[String], summary: &Summary) -> Result<Vec<String>, &'static str> {
    if names.len() > MAX_BACKGROUND {
        return Err("more than 8 processes were labelled background");
    }
    let mut labels: Vec<String> = Vec::new();
    for name in names {
        if *name == summary.target_name {
            return Err("the target cannot be labelled background");
        }
        if !summary.labelable.contains(name) {
            return Err("a background name is not a process the table lets it label");
        }
        if labels.contains(name) {
            return Err("a process was labelled twice");
        }
        labels.push(name.clone());
    }
    Ok(labels)
}

/// Checks the model's answer against the summary it was given. `current` and
/// `current_background` describe what runs now, so asking for exactly that is a
/// keep, not a trial.
pub fn decide(
    completion: &Completion,
    summary: &Summary,
    current: PolicySpec,
    current_background: &[String],
) -> Decision {
    if completion.cut_off {
        return refuse("the answer was cut off before it ended");
    }
    let content = text::without_thinking(&completion.text);
    let Ok(answer) = serde_json::from_str::<Answer>(content) else {
        return refuse("the answer was not the JSON object asked for");
    };
    let reason = answer.reason.trim();
    let bad_reason = reason.is_empty()
        || reason.chars().count() > REASON_MAX_CHARS
        || reason.chars().any(char::is_control);
    if bad_reason {
        return refuse("the reason was not one line of at most 160 characters");
    }
    let reason = reason.to_string();
    match answer.action.as_str() {
        "keep" => {
            return Decision::Keep {
                reason,
                invalid: None,
            };
        }
        "try" => {}
        _ => return refuse("the action was not keep or try"),
    }
    let Ok(spec) = PolicySpec::parse(&answer.policy, &answer.preset) else {
        return refuse("the policy or preset is not in the catalog");
    };
    let background = match check_labels(&answer.background, summary) {
        Ok(labels) => labels,
        Err(why) => return refuse(why),
    };
    if !spec.policy.uses_labels() && !background.is_empty() {
        return refuse("background labels apply only to classes");
    }
    let mut asked = background.clone();
    asked.sort();
    let mut now = current_background.to_vec();
    now.sort();
    let unchanged = spec.policy == current.policy
        && (!spec.policy.uses_labels() || (spec.preset == current.preset && asked == now));
    if unchanged {
        return Decision::Keep {
            reason,
            invalid: Some(String::from("that is what runs now")),
        };
    }
    Decision::Try {
        reason,
        spec,
        background,
    }
}

#[cfg(test)]
mod tests;
