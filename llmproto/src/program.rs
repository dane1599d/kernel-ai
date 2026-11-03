//! Program requests (ADR 0012, spike S12-2): the model writes one Kai C entry
//! for the `c>` prompt instead of a Rhai script. The system prompt is the
//! `c-1` card ([`crate::pack::c1`]); the answer is the script envelope, checked
//! by [`script::answer`] with the same limits. A repair round sends back the
//! program and what went wrong, framed by the caller: the pre-check's problems
//! or wcc's message (with the entry's line numbers), a run's failure, or what
//! it printed.

use crate::chat::{self, Message, Role};
use crate::pack::{self, Format};
use crate::script::{self, Repair};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The system prompt: the `c-1` card.
pub fn system_prompt() -> String {
    pack::render(Format::C1)
}

/// The conversation for `request`: the card and the request, then, for a
/// repair, the refused program and why, as [`script::request`] builds it.
pub fn request(request: &str, repair: Option<Repair<'_>>) -> chat::Request {
    let tag = "\n/no_think";
    let user = format!("Request: {request}{tag}");
    let mut asked = chat::Request::new(Some(&system_prompt()), &user, script::SCRIPT_MAX_TOKENS);
    if let Some(repair) = repair {
        let previous = serde_json::json!({ "script": repair.script, "summary": "" }).to_string();
        let feedback = format!("{}\nWrite a corrected program.{tag}", repair.error);
        asked
            .messages
            .push(Message::new(Role::Assistant, &previous));
        asked.messages.push(Message::new(Role::User, &feedback));
    }
    asked
}

/// The chat body for a server: [`request`]'s conversation under the script
/// envelope's `response_format`, as [`script::chat_request`] builds Rhai's.
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
