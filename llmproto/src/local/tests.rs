//! Requests on generated models: every script the engine writes under the shape
//! is one `script::answer` accepts, or one it refuses for a limit, as it would a
//! server's, whatever the model and the request; the greedy path is the
//! engine's own answer; a stop for time is a local error.

extern crate std;

use super::*;
use crate::script::{REQUEST_MAX_BYTES, ScriptRequest, check_summary, check_text};
use alloc::string::String;
use alloc::vec;
use infer::chat::{Budget, Engine, Step};
use infer::fixture::{self, Spec};
use quant::Tier;

const ROOMY: Budget = Budget {
    kv_bytes: 64 << 20,
    other_bytes: 64 << 20,
};

/// Tiny models with room for the script system prompt, one token per byte, and
/// the local-script model, which takes it in a few hundred and ends its strings.
fn models() -> Vec<Vec<u8>> {
    let roomy = |spec: Spec| {
        fixture::build(&Spec {
            context_length: 4096,
            ..spec
        })
    };
    vec![
        fixture::local_script(&system_prompts()),
        roomy(Spec::tiny()),
        roomy(Spec::local_ask()),
        roomy(Spec {
            seed: 77,
            closes_strings: true,
            ..Spec::local_ask()
        }),
    ]
}

/// Why a script past one of the limits is refused.
const LIMITS: [&str; 5] = [
    "the script is longer than 3072 bytes",
    "the script has more than 40 lines",
    " is longer than 120 characters",
    "the summary must be one line of plain text",
    "the summary is longer than 100 characters",
];

fn requests() -> Vec<ScriptRequest> {
    let ask = |request: &str, repair: Option<(&str, &str)>| ScriptRequest {
        request: String::from(request),
        repair: repair.map(|(script, why)| (String::from(script), String::from(why))),
        timeout_ms: 70_000,
    };
    vec![
        ask("how many cpus are there", None),
        ask("", None),
        ask(&"x".repeat(REQUEST_MAX_BYTES), None),
        ask(
            "été \u{1F600} \"quoted\" \\ <|im_end|> {\"script\":1}",
            None,
        ),
        ask(
            "how much memory is free",
            Some((
                "print(memory_free());",
                "syntax error: Function not found: memory_free",
            )),
        ),
        ask(
            "switch to round robin",
            Some(("(the answer was refused before it was read)", "")),
        ),
    ]
}

#[test]
fn every_script_from_the_shape_parses_or_is_refused_for_a_limit() {
    let (mut answered, mut refused) = (0, 0);
    for bytes in models() {
        let mut engine = Engine::load(&bytes, Tier::best(), 4096, ROOMY).unwrap();
        for request in requests() {
            match script(&mut engine, &request, |_| Next::Continue) {
                // What script::answer returned is what its own checks accept,
                // and needs no trimming to be shown.
                Ok(answer) => {
                    answered += 1;
                    assert_eq!(check_text(&answer.script).as_ref(), Ok(&answer.script));
                    assert_eq!(check_summary(&answer.summary).as_ref(), Ok(&answer.summary));
                }
                Err(AnswerError::Rejected(why)) => {
                    refused += 1;
                    assert!(LIMITS.iter().any(|limit| why.contains(limit)), "{why}");
                }
                Err(AnswerError::NoScript(_) | AnswerError::CutOff) => {}
                Err(other) => panic!("{request:?}: {other:?}"),
            }
        }
    }
    assert!(answered > 0 && refused > 0, "{answered} {refused}");
}

#[test]
fn scripts_are_the_same_every_time_and_on_every_tier() {
    let bytes = &models()[0];
    let request = &requests()[0];
    let answers: Vec<_> = Tier::all()
        .flat_map(|tier| {
            let mut engine = Engine::load(bytes, tier, 4096, ROOMY).unwrap();
            [(); 2].map(|()| script(&mut engine, request, |_| Next::Continue))
        })
        .collect();
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "{answers:?}"
    );
    assert!(answers[0].is_ok(), "{answers:?}");
}

#[test]
fn the_local_script_model_takes_the_prompt_in_few_tokens() {
    // Trained on the system prompts, so a script request fits the 2,048
    // positions of the llm service with room for a repair round.
    let bytes = fixture::local_script(&system_prompts());
    let engine = Engine::load(&bytes, Tier::best(), 2048, ROOMY).unwrap();
    let asked = script::request("show me the cpus please", None);
    let (system, user) = asked.single_turn().unwrap();
    let tokens = engine.prompt_tokens(system, user);
    let bytes = system.unwrap().len() + user.len();
    std::println!("script prompt: {bytes} bytes in {tokens} tokens");
    assert!(tokens * 3 < bytes, "{tokens} tokens for {bytes} bytes");
    assert_eq!(engine.about().context, 2048);
}

#[test]
fn a_stop_for_time_is_a_local_error() {
    let bytes = &models()[0];
    let mut engine = Engine::load(bytes, Tier::best(), 4096, ROOMY).unwrap();
    let mut tokens = 0;
    let stopped = script(&mut engine, &requests()[0], |step| {
        if let Step::Token(_) = step {
            tokens += 1;
        }
        match tokens {
            3 => Next::Stop,
            _ => Next::Continue,
        }
    });
    assert_eq!(
        stopped,
        Err(AnswerError::Local(String::from(
            "it ran out of time before the script was complete"
        )))
    );
    // Too small a context is said plainly, and not repaired.
    let small = fixture::build(&Spec::tiny());
    let mut engine = Engine::load(&small, Tier::best(), 512, ROOMY).unwrap();
    let refused = script(&mut engine, &requests()[0], |_| Next::Continue).unwrap_err();
    assert_eq!(
        refused,
        AnswerError::Local(String::from(
            "the request is too long for the model's context"
        ))
    );
    assert!(!refused.repairable());
    // So is an answer the context ran out under, which is no token limit: a
    // model of plain words never closes its script in 40 positions.
    let words = &models()[2];
    let asked = script::request(&requests()[0].request, None);
    let (system, user) = asked.single_turn().unwrap();
    let prompt = Engine::load(words, Tier::best(), 4096, ROOMY)
        .unwrap()
        .prompt_tokens(system, user);
    let mut engine = Engine::load(words, Tier::best(), prompt + 40, ROOMY).unwrap();
    let full = script(&mut engine, &requests()[0], |_| Next::Continue).unwrap_err();
    assert_eq!(
        full,
        AnswerError::Local(String::from("the conversation filled the model's context"))
    );
    assert!(!full.repairable());
}

#[test]
fn completions_are_the_engines_greedy_answers() {
    let bytes = &models()[0];
    let mut engine = Engine::load(bytes, Tier::best(), 4096, ROOMY).unwrap();
    let asked = chat::ask("What is a kernel?");
    let completion = complete(&mut engine, &asked, |_| Next::Continue).unwrap();
    let reply = engine
        .answer(
            Some(chat::ASK_SYSTEM_PROMPT),
            "What is a kernel?",
            256,
            |_| Next::Continue,
        )
        .unwrap();
    assert_eq!(completion.text, reply.text());
    assert_eq!(completion.cut_off, reply.cut_off());
    // A conversation of more than one turn is rendered whole.
    let repaired = script::request(
        "how many cpus",
        Some(script::Repair {
            script: "cpus",
            error: "no",
        }),
    );
    assert_eq!(turns(&repaired).len(), 4);
    assert_eq!(
        turns(&repaired)
            .iter()
            .map(|turn| turn.role.name())
            .collect::<Vec<_>>(),
        ["system", "user", "assistant", "user"]
    );
}
