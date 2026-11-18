//! The mock's answer to an app round (`app_build`'s `c-app-1` request, "Kai
//! builds apps"): the card's own worked example, a shopping list, which
//! llmproto's tests hold to `kaiapp.h` and `cc` builds, so the app a boot case
//! asks for builds, passes its trial and installs. A repair round, whose card
//! leaves the example out, gets the same source back.

use llmproto::pack::capp1;
use serde_json::{Value, json};

/// The line the `app-build` case types, and the call the mock makes for it.
pub(super) const BUILD_LINE: &str = "build me a shopping list app";
pub(super) const BUILD_CALL: &str =
    r#"{"name":"app_build","arguments":{"name":"shopping","request":"a shopping list"}}"#;

/// Whether `request` is an app round: its system prompt is the `c-app-1`
/// card, whole or as a repair's.
pub(super) fn is_app(request: &Value) -> bool {
    let system = &request["messages"][0]["content"];
    *system == llmproto::app::system_prompt().as_str() || *system == capp1::render_repair().as_str()
}

/// The script envelope with the card's example as `main.c`.
pub(super) fn app() -> String {
    let (_, source, summary) = capp1::EXAMPLES[0];
    json!({ "script": source, "summary": summary }).to_string()
}

#[cfg(test)]
mod tests {
    use super::super::answer;
    use super::*;
    use llmproto::openai::parse_completion;
    use llmproto::script::{self, Lang, Repair};

    fn ask(repair: Option<Repair<'_>>) -> script::Answer {
        let body = llmproto::app::chat_request("mock", "shopping\na shopping list", repair);
        let reply = answer(&body).unwrap();
        let completion = parse_completion(reply.as_bytes()).unwrap();
        script::answer_within(&completion, Lang::App.limits()).unwrap()
    }

    /// An app round, built as llm's server backend builds it, is answered
    /// with the card's example in the envelope `app_build` reads, first time
    /// and on a repair.
    #[test]
    fn an_app_round_is_answered_with_the_cards_example() {
        let first = ask(None);
        // The envelope's script is read back trimmed.
        assert_eq!(first.script, capp1::EXAMPLES[0].1.trim_end());
        assert!(first.script.contains("void view("));
        let repair = Repair {
            script: &first.script,
            error: "cc: error: something",
        };
        assert_eq!(ask(Some(repair)).script, first.script);
    }

    #[test]
    fn the_build_line_calls_app_build() {
        let call = llmproto::act::parse(BUILD_CALL).unwrap();
        assert_eq!(call.name, "app_build");
        assert_eq!(call.arguments["name"], "shopping");
    }
}
