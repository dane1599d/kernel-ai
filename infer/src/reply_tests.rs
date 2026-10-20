//! Whole replies from generated models under a JSON shape: each closes the object,
//! stops at a limit or is cut off just after the token that left too little room,
//! and a closed one parses and keeps its limits; the same on every tier; a stop is
//! a stop. Greedy replies without a shape are greedy generation on the
//! conversation's prompt, and seeded ones are not greedy.

use crate::chat::{Budget, Ending, Engine, Reply, Role, Step, Turn};
use crate::fixture::{self, Spec};
use crate::json::{Grammar, Limit};
use crate::json_tests::{SCRIPT_SHAPE, assert_fits};
use crate::{Error, Next, Sampler, generate};
use alloc::vec::Vec;
use bpe::Tokenizer;
use bpe::chat::Chat;
use quant::Tier;

const ROOMY: Budget = Budget {
    kv_bytes: 64 << 20,
    other_bytes: 64 << 20,
};

fn script_turns(request: &str) -> [Turn<'_>; 2] {
    [
        Turn::new(Role::System, "Write a script as JSON."),
        Turn::new(Role::User, request),
    ]
}

/// A model whose output reaches only words and the quote, which ends strings.
fn closing(seed: u64) -> Spec {
    Spec {
        context_length: 512,
        untied: true,
        printable: true,
        closes_strings: true,
        seed,
        ..Spec::tiny()
    }
}

/// Checks a shaped reply ended as the rules say, given the prompt's length and
/// the limits it ran under: a closed object fits; a cut-off came with the token
/// after which the object could no longer close, and not before.
fn assert_ended_right(reply: &Reply, prompt: usize, max_new: usize, context: usize) {
    let grammar = Grammar::new(&SCRIPT_SHAPE).unwrap();
    let left = |made: usize| (max_new - made).min(context + 1 - prompt - made);
    match reply.ending {
        Ending::EndToken => assert_fits(&SCRIPT_SHAPE, &reply.bytes),
        Ending::MaxTokens | Ending::ContextFull => {
            let at = grammar.advance(grammar.start(), &reply.bytes).unwrap();
            assert!(grammar.need(&at) > left(reply.tokens), "{reply:?}");
            assert!(reply.tokens > 0);
        }
        Ending::Overflow(_) => assert!(reply.cut_off()),
        Ending::Stopped => panic!("nothing stopped {reply:?}"),
    }
    assert!(reply.tokens <= max_new && prompt + reply.tokens <= context + 1);
}

#[test]
fn generated_models_close_stop_or_are_cut_off() {
    let mut endings = Vec::new();
    for spec in [Spec::tiny(), Spec::local_ask(), closing(4), closing(5)] {
        let bytes = fixture::build(&Spec {
            context_length: 512,
            ..spec
        });
        let mut engine = Engine::load(&bytes, Tier::best(), 512, ROOMY).unwrap();
        for (i, request) in ["hi", "how many cpus", "the policy please", ""]
            .iter()
            .enumerate()
        {
            let turns = script_turns(request);
            let prompt = engine.prompt_tokens(Some(turns[0].text), turns[1].text);
            for max_new in [26, 27, 40, 300] {
                let mut sampler = Sampler::seeded(0.2, 42 + i as u64);
                let reply = engine
                    .reply(&turns, max_new, &mut sampler, Some(&SCRIPT_SHAPE), |_| {
                        Next::Continue
                    })
                    .unwrap();
                assert_ended_right(&reply, prompt, max_new, 512);
                endings.push(reply.ending);
            }
        }
        // One token too few is refused before anything runs.
        let refused = engine.reply(
            &script_turns("hi"),
            25,
            &mut Sampler::greedy(),
            Some(&SCRIPT_SHAPE),
            |_| panic!("nothing runs"),
        );
        assert_eq!(refused.err(), Some(Error::NoRoom));
    }
    // Objects closed, answers were cut off, and lines overflowed.
    assert!(endings.contains(&Ending::EndToken), "{endings:?}");
    assert!(endings.contains(&Ending::MaxTokens), "{endings:?}");
    assert!(endings.iter().any(|e| matches!(e, Ending::Overflow(_))));
}

#[test]
fn a_model_that_writes_plain_words_overflows_its_line() {
    // local-ask's model writes only words, so its script never ends by itself.
    let bytes = fixture::build(&Spec {
        context_length: 512,
        ..Spec::local_ask()
    });
    let mut engine = Engine::load(&bytes, Tier::best(), 512, ROOMY).unwrap();
    let reply = engine
        .reply(
            &script_turns("hi"),
            300,
            &mut Sampler::greedy(),
            Some(&SCRIPT_SHAPE),
            |_| Next::Continue,
        )
        .unwrap();
    let Ending::Overflow(overflow) = reply.ending else {
        panic!("{:?} after {} tokens", reply.ending, reply.tokens);
    };
    assert_eq!(
        (overflow.field, overflow.limit, overflow.line),
        (0, Limit::LineChars, 1)
    );
    assert!(reply.cut_off());
    assert!(reply.bytes.len() <= 11 + 120);
}

#[test]
fn the_context_and_the_token_limit_cut_off_what_they_cannot_hold() {
    let bytes = fixture::build(&closing(6));
    let turns = script_turns("hi");
    let prompt = Engine::load(&bytes, Tier::best(), 512, ROOMY)
        .unwrap()
        .prompt_tokens(Some(turns[0].text), turns[1].text);
    // Room for the shortest object: 26 tokens, all but the last in the context.
    let mut endings = Vec::new();
    for room in 24..60 {
        let context = prompt + room;
        let mut engine = Engine::load(&bytes, Tier::best(), context, ROOMY).unwrap();
        for max_new in [26, 30, 500] {
            let mut sampler = Sampler::seeded(0.2, room as u64);
            let reply = engine.reply(&turns, max_new, &mut sampler, Some(&SCRIPT_SHAPE), |_| {
                Next::Continue
            });
            if room < 25 {
                assert_eq!(reply.err(), Some(Error::NoRoom));
                continue;
            }
            let reply = reply.unwrap();
            assert_ended_right(&reply, prompt, max_new, context);
            endings.push(reply.ending);
        }
    }
    assert!(endings.contains(&Ending::ContextFull), "{endings:?}");
    assert!(endings.contains(&Ending::MaxTokens), "{endings:?}");
    assert!(endings.contains(&Ending::EndToken), "{endings:?}");
}

/// The shaped reply to "how many cpus" on `tier`, stopped after `stop_at` tokens.
fn cpus(bytes: &[u8], tier: Tier, stop_at: Option<usize>) -> Reply {
    let mut engine = Engine::load(bytes, tier, 512, ROOMY).unwrap();
    let (mut made, mut sampler) = (0, Sampler::seeded(0.2, 42));
    engine
        .reply(
            &script_turns("how many cpus"),
            200,
            &mut sampler,
            Some(&SCRIPT_SHAPE),
            |step| {
                made += matches!(step, Step::Token(_)) as usize;
                match Some(made) == stop_at {
                    true => Next::Stop,
                    false => Next::Continue,
                }
            },
        )
        .unwrap()
}

/// A model that closes its object for "how many cpus": the first seed that does.
fn closes() -> Vec<u8> {
    (1..40)
        .map(|seed| fixture::build(&closing(seed)))
        .find(|bytes| cpus(bytes, Tier::best(), None).ending == Ending::EndToken)
        .expect("a seed whose model closes its object")
}

#[test]
fn every_tier_writes_the_same_object() {
    let bytes = closes();
    let answers: Vec<_> = Tier::all().map(|tier| cpus(&bytes, tier, None)).collect();
    assert!(!answers.is_empty());
    assert!(answers.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(answers[0].ending, Ending::EndToken);
}

#[test]
fn a_stop_mid_answer_is_a_stop_and_a_closed_object_is_not() {
    let bytes = closes();
    let full = cpus(&bytes, Tier::best(), None);
    assert_eq!(full.ending, Ending::EndToken);
    assert!(full.tokens > 5, "{full:?}");
    let stopped = cpus(&bytes, Tier::best(), Some(5));
    assert_eq!((stopped.ending, stopped.tokens), (Ending::Stopped, 5));
    // A stop asked for with the token that closes the object comes too late.
    assert_eq!(cpus(&bytes, Tier::best(), Some(full.tokens)), full);
}

/// The fixture's tokenizer and chat, as the engine builds them.
fn chat_of(bytes: &[u8]) -> (Tokenizer, Chat) {
    let file = gguf::Gguf::parse(bytes).unwrap();
    let tok = Tokenizer::from_gguf(file.header()).unwrap();
    let chat = Chat::new(&tok).unwrap();
    (tok, chat)
}

#[test]
fn greedy_replies_are_greedy_generation_on_the_conversation() {
    let bytes = fixture::build(&Spec::tiny());
    let (tok, chat) = chat_of(&bytes);
    let turns = [
        Turn::new(Role::System, "Be brief."),
        Turn::new(Role::User, "hi"),
        Turn::new(Role::Assistant, "{\"script\":\"x\"}"),
        Turn::new(Role::User, "again"),
    ];
    let prompt = chat.conversation(&tok, &turns);
    let file = gguf::Gguf::parse(&bytes).unwrap();
    let mut model = crate::Model::new(&file, Tier::BASELINE, 512).unwrap();
    let mut kv = model.kv_cache().unwrap();
    let mut expected = Vec::new();
    generate(&mut model, &mut kv, &prompt, 40, |token| {
        if chat.is_stop(token) {
            return Next::Stop;
        }
        expected.push(token);
        Next::Continue
    })
    .unwrap();
    let mut engine = Engine::load(&bytes, Tier::BASELINE, 512, ROOMY).unwrap();
    let tokens = |engine: &mut Engine<'_>, sampler: &mut Sampler| {
        let mut seen = Vec::new();
        let reply = engine
            .reply(&turns, 40, sampler, None, |step| {
                if let Step::Token(token) = step {
                    seen.push(token);
                }
                Next::Continue
            })
            .unwrap();
        assert_eq!(reply.prompt_tokens, prompt.len());
        seen
    };
    assert_eq!(tokens(&mut engine, &mut Sampler::greedy()), expected);
    // Seeded sampling draws from the whole vocabulary, the same way every time,
    // and not the greedy way.
    let drawn = tokens(&mut engine, &mut Sampler::seeded(0.9, 5));
    assert_eq!(tokens(&mut engine, &mut Sampler::seeded(0.9, 5)), drawn);
    assert_ne!(drawn, expected);
    // Under a shape too.
    let shaped = |engine: &mut Engine<'_>, sampler: &mut Sampler| {
        engine
            .reply(&turns, 60, sampler, Some(&SCRIPT_SHAPE), |_| Next::Continue)
            .unwrap()
            .bytes
    };
    let greedy = shaped(&mut engine, &mut Sampler::greedy());
    assert_ne!(shaped(&mut engine, &mut Sampler::seeded(0.9, 5)), greedy);
}
