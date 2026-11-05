//! App requests (plan "Kai builds apps", piece 2): the model writes a whole
//! window app's `main.c` for `app_build`. The system prompt is the `c-app-1`
//! card ([`crate::pack::capp1`]); the answer is the script envelope, checked by
//! [`script::answer_within`] under [`script::APP_LIMITS`]. A repair round
//! sends back the source and why the shell refused it: the pre-check's
//! problems or `cc build`'s diagnostics.

use crate::chat::{self, Message, Role};
use crate::pack::capp1;
use crate::script::{self, Repair};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The system prompt: the `c-app-1` card.
pub fn system_prompt() -> String {
    capp1::render()
}

/// The user's turn for an `app` request's text, `<name>\n<request>` as the
/// shell sends it: the name the app installs under, then the user's words.
pub fn user_turn(request: &str) -> String {
    match request.split_once('\n') {
        Some((name, words)) => format!("App name: {name}\nRequest: {words}"),
        None => format!("Request: {request}"),
    }
}

/// The conversation for `request`: the card and the request, then, for a
/// repair, the refused source and why.
pub fn request(request: &str, repair: Option<Repair<'_>>) -> chat::Request {
    let tag = "\n/no_think";
    let user = format!("{}{tag}", user_turn(request));
    // A repair's card leaves out the example, so the refused source and a
    // whole corrected main.c fit in the context with it.
    let system = match repair {
        Some(_) => capp1::render_repair(),
        None => system_prompt(),
    };
    let mut asked = chat::Request::new(Some(&system), &user, script::APP_MAX_TOKENS);
    if let Some(repair) = repair {
        let previous = serde_json::json!({ "script": repair.script, "summary": "" }).to_string();
        let feedback = format!("{}\nWrite the corrected main.c.{tag}", repair.error);
        asked
            .messages
            .push(Message::new(Role::Assistant, &previous));
        asked.messages.push(Message::new(Role::User, &feedback));
    }
    asked
}

/// The chat body for a server: [`request`]'s conversation under the script
/// envelope's `response_format`.
pub fn chat_request(model: &str, request: &str, repair: Option<Repair<'_>>) -> Vec<u8> {
    let asked = self::request(request, repair);
    let body = serde_json::json!({
        "model": model,
        "messages": asked.messages,
        "max_tokens": asked.max_tokens,
        "temperature": script::TEMPERATURE,
        "seed": script::SEED,
        "stream": false,
        "response_format": script::response_format(),
        "chat_template_kwargs": { "enable_thinking": false },
    });
    serde_json::to_vec(&body).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card says the limits the answer is held to.
    #[test]
    fn the_card_states_the_app_limits() {
        let card = system_prompt();
        assert!(card.contains(&format!(
            "At most {} lines of {} characters",
            script::APP_MAX_LINES,
            script::SCRIPT_MAX_LINE_CHARS
        )));
    }

    /// A repair keeps the card's rules, calls and template but not its
    /// example, which frees the room a whole corrected main.c needs.
    #[test]
    fn a_repair_card_leaves_out_the_example() {
        let first = request("guess\na guessing game", None);
        let repair = Repair {
            script: "int main;",
            error: "cc build refused it",
        };
        let again = request("guess\na guessing game", Some(repair));
        let (card, short) = (&first.messages[0].content, &again.messages[0].content);
        assert!(card.contains("Example:") && !short.contains("Example:"));
        assert!(card.starts_with(short.as_str()));
        assert!(short.contains("Template:"));
        assert!(capp1::estimate_tokens(card) - capp1::estimate_tokens(short) > 250);
        assert_eq!(again.messages.len(), 4);
    }

    #[test]
    fn the_name_and_the_words_are_their_own_lines() {
        assert_eq!(
            user_turn("timer\na countdown timer"),
            "App name: timer\nRequest: a countdown timer"
        );
        assert_eq!(user_turn("a timer"), "Request: a timer");
    }

    #[test]
    fn a_repair_carries_the_source_and_why() {
        let repair = Repair {
            script: "#include <kaiapp.h>\n",
            error: "cc build refused it:\nmain.c(2): undeclared",
        };
        let body: serde_json::Value =
            serde_json::from_slice(&chat_request("m", "timer\na timer", Some(repair))).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["content"], capp1::render_repair());
        assert!(
            messages[1]["content"]
                .as_str()
                .unwrap()
                .contains("App name: timer")
        );
        assert!(
            messages[2]["content"]
                .as_str()
                .unwrap()
                .contains("kaiapp.h")
        );
        assert!(
            messages[3]["content"]
                .as_str()
                .unwrap()
                .contains("undeclared")
        );
        assert_eq!(body["max_tokens"], script::APP_MAX_TOKENS);
    }

    #[test]
    fn an_app_answer_may_be_longer_than_a_script() {
        let source = "x;\n".repeat(script::SCRIPT_MAX_LINES + 10);
        let envelope = serde_json::json!({ "script": source, "summary": "An app." }).to_string();
        let completion = chat::Completion {
            text: envelope,
            cut_off: false,
        };
        assert!(script::answer(&completion).is_err());
        assert!(script::answer_within(&completion, script::APP_LIMITS).is_ok());
        let long = "x;\n".repeat(script::APP_MAX_LINES + 1);
        let envelope = serde_json::json!({ "script": long, "summary": "An app." }).to_string();
        let completion = chat::Completion {
            text: envelope,
            cut_off: false,
        };
        assert!(script::answer_within(&completion, script::APP_LIMITS).is_err());
    }
}
