//! One answer through [`chat::Engine`]: the same tokens as greedy generation, the
//! callback after every step, each way an answer ends, the memory budget, and the
//! local-ask fixture's properties.

extern crate std;

use crate::chat::{Budget, Ending, Engine, LoadError, MIN_CONTEXT, Step};
use crate::fixture::{self, Spec};
use crate::*;
use alloc::vec::Vec;
use gguf::Gguf;
use quant::Tier;

const ROOMY: Budget = Budget {
    kv_bytes: 64 << 20,
    other_bytes: 64 << 20,
};
const SYSTEM: &str = "You are a test.";

fn engine(bytes: &[u8], context_cap: usize) -> Engine<'_> {
    Engine::load(bytes, Tier::best(), context_cap, ROOMY).unwrap()
}

/// The answer and every token the callback saw, with no stop.
fn answer(engine: &mut Engine<'_>, question: &str, max_new: usize) -> (chat::Reply, Vec<u32>) {
    let mut seen = Vec::new();
    let reply = engine
        .answer(Some(SYSTEM), question, max_new, |step| {
            if let Step::Token(token) = step {
                seen.push(token);
            }
            Next::Continue
        })
        .unwrap();
    (reply, seen)
}

#[test]
fn an_answer_is_greedy_generation_on_the_chat_prompt() {
    let bytes = fixture::build(&Spec::tiny());
    let mut engine = engine(&bytes, 128);
    let (reply, seen) = answer(&mut engine, "What does a kernel do?", 24);
    assert_eq!(reply.ending, Ending::MaxTokens);
    assert_eq!(reply.tokens, 24);
    let file = Gguf::parse(&bytes).unwrap();
    let tok = bpe::Tokenizer::from_gguf(file.header()).unwrap();
    let chat = bpe::chat::Chat::new(&tok).unwrap();
    let prompt = chat.prompt(&tok, Some(SYSTEM), "What does a kernel do?");
    assert_eq!(reply.prompt_tokens, prompt.len());
    let mut model = Model::new(&file, Tier::BASELINE, 128).unwrap();
    let mut kv = model.kv_cache().unwrap();
    let mut want = Vec::new();
    generate(&mut model, &mut kv, &prompt, 24, |token| {
        want.push(token);
        Next::Continue
    })
    .unwrap();
    assert_eq!(seen, want);
    let mut text = Vec::new();
    tok.decode(&want, &mut text);
    assert_eq!(reply.bytes, text);
    // A second question starts from an empty context and gets the same answer.
    assert_eq!(answer(&mut engine, "What does a kernel do?", 24).1, want);
}

#[test]
fn the_callback_runs_after_every_step_and_can_stop_anywhere() {
    let bytes = fixture::build(&Spec::tiny());
    let mut engine = engine(&bytes, 128);
    let (mut prompted, mut made) = (0, 0);
    let reply = engine
        .answer(None, "hi", 5, |step| {
            match step {
                Step::Prompt => prompted += 1,
                Step::Token(_) => made += 1,
            }
            Next::Continue
        })
        .unwrap();
    assert_eq!((prompted, made), (reply.prompt_tokens, reply.tokens));
    let mut steps = 0;
    let stopped = engine
        .answer(None, "hi", 5, |_| {
            steps += 1;
            if steps == 3 {
                Next::Stop
            } else {
                Next::Continue
            }
        })
        .unwrap();
    assert_eq!((stopped.ending, stopped.tokens), (Ending::Stopped, 0));
    assert!(stopped.cut_off());
    let mut tokens = 0;
    let early = engine
        .answer(None, "hi", 5, |step| {
            tokens += matches!(step, Step::Token(_)) as usize;
            if tokens == 2 {
                Next::Stop
            } else {
                Next::Continue
            }
        })
        .unwrap();
    assert_eq!((early.ending, early.tokens), (Ending::Stopped, 2));
}

#[test]
fn the_context_ends_an_answer_and_a_longer_prompt_is_refused() {
    // Its output never reaches an end token.
    let bytes = fixture::local_ask();
    let mut engine = engine(&bytes, MIN_CONTEXT);
    let prompt = engine.prompt_tokens(None, "hi");
    assert!(prompt + 4 < MIN_CONTEXT, "{prompt}");
    let (reply, _) = answer_without_system(&mut engine, "hi", 1000);
    assert_eq!(reply.ending, Ending::ContextFull);
    // The last token made needs no position of its own.
    assert_eq!(reply.tokens, MIN_CONTEXT - prompt + 1);
    let long = "word ".repeat(40);
    let refused = engine.answer(None, &long, 8, |_| Next::Continue);
    assert_eq!(refused, Err(Error::ContextFull));
    // The engine still answers after a refusal.
    assert_eq!(answer_without_system(&mut engine, "hi", 1000).0, reply);
}

fn answer_without_system(
    engine: &mut Engine<'_>,
    question: &str,
    max_new: usize,
) -> (chat::Reply, Vec<u32>) {
    let mut seen = Vec::new();
    let reply = engine
        .answer(None, question, max_new, |step| {
            if let Step::Token(token) = step {
                seen.push(token);
            }
            Next::Continue
        })
        .unwrap();
    (reply, seen)
}

#[test]
fn every_tier_gives_the_same_answer() {
    let bytes = fixture::local_ask();
    let answers: Vec<_> = Tier::all()
        .map(|tier| {
            let mut engine = Engine::load(&bytes, tier, 512, ROOMY).unwrap();
            answer(&mut engine, "hi", 256).0
        })
        .collect();
    assert!(answers.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn the_kv_cache_gets_what_the_budget_holds() {
    let bytes = fixture::build(&Spec::tiny());
    let config = Config::from_gguf(Gguf::parse(&bytes).unwrap().header()).unwrap();
    let per_position = config.kv_bytes(1).unwrap();
    let budget = |positions: usize| Budget {
        kv_bytes: positions * per_position + per_position - 1,
        other_bytes: ROOMY.other_bytes,
    };
    let sized = Engine::load(&bytes, Tier::best(), 512, budget(100)).unwrap();
    assert_eq!(sized.about().context, 100);
    assert_eq!(sized.about().kv_bytes, 100 * per_position);
    // The cap and the trained context bound it too.
    assert_eq!(engine(&bytes, 64).about().context, 64);
    assert_eq!(
        engine(&bytes, 100_000).about().context,
        config.context_length
    );
    assert_eq!(
        Engine::load(&bytes, Tier::best(), 512, budget(MIN_CONTEXT - 1)).err(),
        Some(LoadError::NoContext(MIN_CONTEXT - 1))
    );
}

#[test]
fn a_vocabulary_too_large_for_the_budget_is_refused_before_it_is_built() {
    let bytes = fixture::build(&Spec::tiny());
    let small = Budget {
        kv_bytes: ROOMY.kv_bytes,
        other_bytes: 64 * 1024,
    };
    let refused = Engine::load(&bytes, Tier::best(), 128, small).err();
    assert!(
        matches!(refused, Some(LoadError::TooLarge(n)) if n > 64 * 1024),
        "{refused:?}"
    );
    // A vocabulary as large as Qwen3's fits the budget the llm service gets (its
    // 151,387 merges add about 7 MiB more to the estimate).
    let wide = fixture::build(&Spec {
        vocab: 151_936,
        ..Spec::tiny()
    });
    // kabi::model::KV_BYTES and ENGINE_OTHER_BYTES.
    let service = Budget {
        kv_bytes: 112 << 20,
        other_bytes: 24 << 20,
    };
    let loaded = Engine::load(&wide, Tier::best(), 512, service).unwrap();
    assert_eq!(loaded.about().context, 512);
    assert_eq!(loaded.about().config.vocab, 151_936);
}

#[test]
fn files_that_are_not_models_are_refused_with_their_reason() {
    assert!(matches!(
        Engine::load(b"GGUF\x03\0\0\0junk", Tier::best(), 128, ROOMY).err(),
        Some(LoadError::File(_))
    ));
    let bytes = fixture::build(&Spec::tiny());
    let cut = &bytes[..bytes.len() - 1];
    assert!(matches!(
        Engine::load(cut, Tier::best(), 128, ROOMY).err(),
        Some(LoadError::File(gguf::Error::TensorPastEnd))
    ));
}

#[test]
fn the_local_ask_fixture_is_odd_sized_named_and_answers_in_plain_text() {
    let bytes = fixture::local_ask();
    assert_eq!(bytes.len() % 2, 1);
    assert_ne!(bytes.len() % 512, 0);
    assert_eq!(bytes, fixture::local_ask());
    let mut engine = Engine::load(&bytes, Tier::best(), 512, ROOMY).unwrap();
    let about = engine.about().clone();
    assert_eq!(about.name.as_deref(), Some(fixture::LOCAL_ASK_NAME));
    assert_eq!(about.license.as_deref(), Some("MPL-2.0"));
    assert_eq!(about.context, Spec::local_ask().context_length);
    let (reply, seen) = answer(&mut engine, "hi", 256);
    assert_eq!(reply.ending, Ending::ContextFull);
    assert!(reply.tokens >= 8, "{} tokens", reply.tokens);
    assert!(
        reply
            .bytes
            .iter()
            .all(|&b| b == b' ' || b.is_ascii_alphanumeric()),
        "{:?}",
        reply.text()
    );
    let mut distinct = seen.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(distinct.len() >= 3, "only {distinct:?}");
}
