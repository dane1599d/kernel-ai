//! Requests answered by Kernel AI's own engine ([`infer`]), worked out the same
//! way by the llm service inside the OS and by xtask on the host, so what xtask
//! measures and checks is what ships.
//!
//! `ask` and `:why` decode greedily. A script is decoded with
//! [`script::TEMPERATURE`] and [`script::SEED`] under [`SCRIPT_SHAPE`], the JSON
//! envelope a server's `response_format` asks for, so an answer the model closes
//! always parses. Past one of [`script`]'s limits it is refused with the reason
//! [`script::answer`] would give a server's answer, which the shell sends back for
//! a repair; one that runs out of tokens is cut off, and one that fills the
//! context is a local error. The shell checks every answer again, shows it and
//! runs it only after a yes, as it does a server's.

use crate::chat::{self, Completion, Role};
use crate::script::{self, Answer, AnswerError, ScriptRequest};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use infer::chat::{Ending, Engine, Step, Turn};
use infer::json::{Choice, Field, Limit, Overflow, Shape};
use infer::{Next, Sampler};

/// [`script::check_text`]'s limits, which it applies once the end is trimmed,
/// and [`script::check_reply`]'s, which it applies to the summary trimmed at both
/// ends: the longer of the summary's two limits, since the field cannot know yet
/// whether the script was empty. [`script::answer`] holds a script's summary to
/// the shorter one.
const SCRIPT_FIELDS: [Field; 2] = [
    Field {
        name: "script",
        max_chars: script::SCRIPT_MAX_BYTES,
        max_lines: script::SCRIPT_MAX_LINES,
        max_line_chars: script::SCRIPT_MAX_LINE_CHARS,
        trim_start: false,
        hard: false,
    },
    Field {
        name: "summary",
        max_chars: script::REPLY_MAX_CHARS,
        max_lines: 1,
        max_line_chars: script::REPLY_MAX_CHARS,
        trim_start: true,
        hard: false,
    },
];

/// `{"script":"...","summary":"..."}`, script first so the summary is written
/// knowing the code, as [`script::response_format`] asks a server.
pub const SCRIPT_SHAPE: Shape<'static> = Shape {
    fields: &SCRIPT_FIELDS,
    choices: &[],
    // The literals between fields go as one block (jump-forward). They take the
    // tokenizer's own split, which may differ from the model's picks, so an
    // answer can differ from step decoding's; it is the same from a warm cache or
    // a fresh one and on every tier. S02-1 measures the effect on accuracy.
    jump_forward: true,
};

/// One string field whose limits do not apply: the shape makes it a choice.
const fn chosen(name: &'static str) -> Field {
    Field {
        name,
        max_chars: 1,
        max_lines: 1,
        max_line_chars: 1,
        trim_start: false,
        hard: false,
    }
}

/// `tune`'s answer, in [`crate::tune::FIELD_ORDER`]: a one-line reason of at
/// most 160 characters (the reader trims both ends), then the enums.
const TUNE_FIELDS: [Field; 4] = [
    Field {
        name: "reason",
        max_chars: crate::tune::REASON_MAX_CHARS,
        max_lines: 1,
        max_line_chars: crate::tune::REASON_MAX_CHARS,
        trim_start: true,
        hard: true,
    },
    chosen("policy"),
    chosen("preset"),
    chosen("background"),
];
const POLICIES: [&str; 4] = ["interactive", "round-robin", "classes", "responsive"];
const PRESETS: [&str; 2] = ["gentle", "firm"];

/// The system prompts the llm service sends a local model, for a generated test
/// model whose vocabulary is trained on them (`infer::fixture::local_script`).
pub fn system_prompts() -> String {
    [
        script::system_prompt().as_str(),
        crate::explain::SYSTEM_PROMPT,
        chat::ASK_SYSTEM_PROMPT,
    ]
    .join("\n")
}

/// A request's conversation as the engine's turns.
fn turns(request: &chat::Request) -> Vec<Turn<'_>> {
    request
        .messages
        .iter()
        .map(|message| {
            let role = match message.role {
                Role::System => infer::chat::Role::System,
                Role::User => infer::chat::Role::User,
                Role::Assistant => infer::chat::Role::Assistant,
            };
            Turn::new(role, &message.content)
        })
        .collect()
}

/// The engine's greedy answer to `request`, which `on_step` may cut short.
pub fn complete(
    engine: &mut Engine<'_>,
    request: &chat::Request,
    on_step: impl FnMut(Step) -> Next,
) -> Result<Completion, infer::Error> {
    let reply = engine.reply(
        &turns(request),
        request.max_tokens as usize,
        &mut Sampler::greedy(),
        None,
        on_step,
    )?;
    Ok(Completion {
        text: reply.text(),
        cut_off: reply.cut_off(),
    })
}

/// A script for `request`, from the same conversation a server gets
/// ([`script::request`]), checked by [`script::answer`]. `on_step` stops it only
/// when time is up.
pub fn script(
    engine: &mut Engine<'_>,
    request: &ScriptRequest,
    on_step: impl FnMut(Step) -> Next,
) -> Result<Answer, AnswerError> {
    let repair = request
        .repair
        .as_ref()
        .map(|(script, error)| script::Repair { script, error });
    let asked = script::request(&request.request, repair);
    let mut sampler = Sampler::seeded(script::TEMPERATURE, script::SEED);
    let reply = engine
        .reply(
            &turns(&asked),
            asked.max_tokens as usize,
            &mut sampler,
            Some(&SCRIPT_SHAPE),
            on_step,
        )
        .map_err(|error| AnswerError::Local(describe(&error)))?;
    match reply.ending {
        Ending::Stopped => Err(AnswerError::Local(String::from(
            "it ran out of time before the script was complete",
        ))),
        Ending::Overflow(overflow) => Err(AnswerError::Rejected(past(&overflow))),
        // A repair of a long script can leave the answer too little of the
        // context; that is no token limit, so it is said as it is.
        Ending::ContextFull => Err(AnswerError::Local(String::from(
            "the conversation filled the model's context",
        ))),
        _ => script::answer(&Completion {
            text: reply.text(),
            cut_off: reply.cut_off(),
        }),
    }
}

/// Computes the script system prompt into `engine`'s cache, so the next script
/// request starts from it instead of paying for it inside its own time limit.
/// It answers the system turn alone with one token, which the next request's
/// user turn replaces. `on_step` may stop it early; the keys and values computed
/// by then stay in the cache.
pub fn warm(
    engine: &mut Engine<'_>,
    on_step: impl FnMut(Step) -> Next,
) -> Result<(), infer::Error> {
    let system = script::system_prompt();
    let turns = [Turn::new(infer::chat::Role::System, &system)];
    engine
        .reply(&turns, 1, &mut Sampler::greedy(), None, on_step)
        .map(|_| ())
}

/// The model's answer to a `tune` request, greedy, under part 1's schema: the
/// reason, `action`, `policy` and `preset` as enums, and `background` a list of
/// at most 8 of the names the request may label. The shell checks it again with
/// [`crate::tune::decide`]; a reason past its limit ends the answer, which the
/// reader then refuses, so it changes nothing.
pub fn tune(
    engine: &mut Engine<'_>,
    request: &crate::tune::TuneRequest,
    on_step: impl FnMut(Step) -> Next,
) -> Result<Completion, infer::Error> {
    let names: Vec<&str> = request
        .labelable
        .iter()
        .map(String::as_str)
        .take(infer::json::MAX_WORDS)
        .collect();
    let choices = [
        Choice {
            field: 1,
            words: &POLICIES,
            list: None,
        },
        Choice {
            field: 2,
            words: &PRESETS,
            list: None,
        },
        Choice {
            field: 3,
            words: &names,
            list: Some(crate::tune::MAX_BACKGROUND),
        },
    ];
    let shape = Shape {
        fields: &TUNE_FIELDS,
        choices: &choices,
        // Off, so `tune` answers stay the ones M10's recording and `tune check`
        // were made with.
        jump_forward: false,
    };
    let asked = request.chat();
    let reply = engine.reply(
        &turns(&asked),
        asked.max_tokens as usize,
        &mut Sampler::greedy(),
        Some(&shape),
        on_step,
    )?;
    Ok(Completion {
        text: reply.text(),
        cut_off: reply.cut_off() || matches!(reply.ending, Ending::Stopped),
    })
}

/// Why an answer that went past a limit is refused, in [`script::check_text`]'s
/// and [`script::check_summary`]'s words where they can be: the counts they
/// quote are not known when the answer stops.
fn past(overflow: &Overflow) -> String {
    match (overflow.field, overflow.limit) {
        (0, Limit::Chars) => format!(
            "the script is longer than {} bytes",
            script::SCRIPT_MAX_BYTES
        ),
        (0, Limit::Lines) => format!(
            "the script has more than {} lines",
            script::SCRIPT_MAX_LINES
        ),
        (0, Limit::LineChars) => format!(
            "line {} is longer than {} characters",
            overflow.line,
            script::SCRIPT_MAX_LINE_CHARS
        ),
        (_, Limit::Lines) => String::from("the summary must be one line of plain text"),
        _ => format!(
            "the summary is longer than {} characters",
            script::REPLY_MAX_CHARS
        ),
    }
}

/// What went wrong in the engine, in plain words.
pub fn describe(error: &infer::Error) -> String {
    match error {
        infer::Error::ContextFull => "the request is too long for the model's context".into(),
        infer::Error::NoRoom => "the request leaves the model no room to answer".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
