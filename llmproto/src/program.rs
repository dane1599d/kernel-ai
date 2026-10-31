//! Program requests (ADR 0012, spike S12-2): the model writes one Kai C entry
//! for the `c>` prompt instead of a Rhai script. The system prompt is the
//! `c-1` card ([`crate::pack::c1`]); the answer is the script envelope, checked
//! by [`script::answer`] with the same limits. The repair round sends back the
//! refused program and why: wcc's message, with the entry's line numbers.

use crate::chat::{self, Message, Role};
use crate::pack::{self, Format};
use crate::script::{self, Repair};
use alloc::format;
use alloc::string::{String, ToString};

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
        let feedback = format!(
            "That program was refused: {}\nWrite a corrected program.{tag}",
            repair.error
        );
        asked
            .messages
            .push(Message::new(Role::Assistant, &previous));
        asked.messages.push(Message::new(Role::User, &feedback));
    }
    asked
}
