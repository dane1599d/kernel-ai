//! The prompt cache: an engine that has answered before gives exactly the reply a
//! fresh engine gives, on every tier, however its requests share, stop or fail,
//! while running only the tokens after what they share.

use crate::chat::{Budget, Engine, Reply, Role, Step, Turn};
use crate::fixture::{self, Spec};
use crate::json::Shape;
use crate::json_tests::SCRIPT_SHAPE;
use crate::{Config, Error, KvCache, Next, Sampler};
use alloc::vec::Vec;
use quant::Tier;

const ROOMY: Budget = Budget {
    kv_bytes: 64 << 20,
    other_bytes: 64 << 20,
};
const SYSTEM: &str = "Write a script as JSON. Keep it short.";

/// One request of a session: its turns, its token limit, a shape or not, and
/// the answer token after which the callback stops it, if any.
#[derive(Clone, Copy)]
struct Asked {
    system: Option<&'static str>,
    user: &'static str,
    repair: bool,
    max_new: usize,
    shaped: bool,
    /// Under the shape, with jump-forward.
    jump: bool,
    stop_at: Option<usize>,
}

/// [`SCRIPT_SHAPE`] with jump-forward on.
const JUMP_SHAPE: Shape<'static> = Shape {
    jump_forward: true,
    ..SCRIPT_SHAPE
};

const fn asked(system: Option<&'static str>, user: &'static str) -> Asked {
    Asked {
        system,
        user,
        repair: false,
        max_new: 60,
        shaped: true,
        jump: false,
        stop_at: None,
    }
}

impl Asked {
    fn turns(&self) -> Vec<Turn<'static>> {
        let mut turns = Vec::new();
        if let Some(system) = self.system {
            turns.push(Turn::new(Role::System, system));
        }
        turns.push(Turn::new(Role::User, self.user));
        if self.repair {
            turns.push(Turn::new(
                Role::Assistant,
                "{\"script\":\"x\",\"summary\":\"\"}",
            ));
            turns.push(Turn::new(Role::User, "That was refused: x"));
        }
        turns
    }
}

/// The reply to `asked`, checking the callback saw what the reply says.
fn ask(engine: &mut Engine<'_>, asked: &Asked) -> Result<Reply, Error> {
    let mut sampler = Sampler::seeded(0.2, 42);
    let shape = match asked.jump {
        true => &JUMP_SHAPE,
        false => &SCRIPT_SHAPE,
    };
    let shape = asked.shaped.then_some(shape);
    let (mut made, mut cached, mut computed) = (0, None, 0);
    let reply = engine.reply(&asked.turns(), asked.max_new, &mut sampler, shape, |step| {
        match step {
            Step::Cached(n) => cached = Some(n),
            Step::Prompt => computed += 1,
            Step::Token(_) => made += 1,
        }
        match asked.stop_at == Some(made) && made > 0 {
            true => Next::Stop,
            false => Next::Continue,
        }
    });
    if let Ok(reply) = &reply {
        assert_eq!(cached, Some(reply.cached));
        assert_eq!(computed + reply.cached, reply.prompt_tokens);
    }
    reply
}

/// The same reply but for what the cache held.
fn same(warm: &Result<Reply, Error>, fresh: &Result<Reply, Error>) -> bool {
    match (warm, fresh) {
        (Ok(warm), Ok(fresh)) => {
            Reply {
                cached: 0,
                ..warm.clone()
            } == *fresh
        }
        (warm, fresh) => warm == fresh,
    }
}

const SESSION: [Asked; 9] = [
    asked(Some(SYSTEM), "how many cpus"),
    asked(Some(SYSTEM), "how many cpus"),
    Asked {
        repair: true,
        ..asked(Some(SYSTEM), "how many cpus")
    },
    Asked {
        max_new: 40,
        stop_at: Some(3),
        ..asked(Some(SYSTEM), "what is the policy")
    },
    Asked {
        max_new: 20,
        shaped: false,
        ..asked(None, "hi")
    },
    asked(Some(SYSTEM), "what is the policy"),
    Asked {
        max_new: 30,
        shaped: false,
        stop_at: Some(1),
        ..asked(Some("Something else entirely."), "how many cpus")
    },
    Asked {
        repair: true,
        ..asked(Some(SYSTEM), "how many cpus")
    },
    // Longer than the context: refused before anything runs.
    asked(Some(SYSTEM), LONG),
];
const LONG: &str = "a very long request that cannot fit the context of the engine at all, \
                    a very long request that cannot fit the context of the engine at all, \
                    a very long request that cannot fit the context of the engine at all, \
                    a very long request that cannot fit the context of the engine at all, \
                    a very long request that cannot fit the context of the engine at all";

#[test]
fn a_warm_engine_answers_as_a_fresh_one_on_every_tier() {
    let bytes = fixture::build(&Spec::tiny());
    let fresh = |tier, asked: &Asked| {
        let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        ask(&mut engine, asked)
    };
    for tier in Tier::all() {
        let mut warm = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let mut reused = Vec::new();
        for asked in &SESSION {
            let expected = fresh(tier, asked);
            assert!(expected.as_ref().map_or(true, |reply| reply.cached == 0));
            let got = ask(&mut warm, asked);
            assert!(same(&got, &expected), "{got:?} vs {expected:?}");
            reused.push(got.map_or(0, |reply| reply.cached));
        }
        assert_eq!(fresh(tier, &SESSION[8]), Err(Error::ContextFull));
        let prompt = |asked: &Asked| fresh(tier, asked).unwrap().prompt_tokens;
        // The same request again: all of its prompt but the last token.
        assert_eq!(reused[0], 0);
        assert_eq!(reused[1], prompt(&SESSION[1]) - 1);
        // A repair round: its first turns, up to the assistant's start.
        assert!(reused[2] > prompt(&SESSION[0]) - 8, "{reused:?}");
        // After an ask with no system prompt, little; another system prompt
        // shares only the start of its block.
        assert!(reused[5] < 10 && reused[6] < 10, "{reused:?}");
        // The repair again, after other requests: only what is still shared.
        assert!(reused[7] < 10, "{reused:?}");
    }
}

#[test]
fn with_jump_forward_a_warm_engine_answers_as_a_fresh_one_and_caches_exactly() {
    // The forced literals go to the model as blocks; the answers are the same
    // from a warm cache and a fresh one, and the cache holds, bit for bit, what
    // one token at a time from an empty cache gives.
    let bytes = fixture::build(&Spec::tiny());
    let session = SESSION.map(|asked| Asked {
        jump: true,
        ..asked
    });
    for tier in Tier::all() {
        let mut warm = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let mut forced = 0;
        for asked in &session {
            let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
            let expected = ask(&mut engine, asked);
            let got = ask(&mut warm, asked);
            assert!(same(&got, &expected), "{got:?} vs {expected:?}");
            assert_cache_holds_its_tokens(&bytes, tier, &warm);
            if let Ok(reply) = &got {
                assert!(reply.forced <= reply.tokens);
                forced += reply.forced;
            }
        }
        assert!(forced > 0, "nothing was forced");
    }
}

#[test]
fn jump_forward_forces_the_opening_and_closing_literals() {
    let bytes = fixture::build(&Spec::tiny());
    let mut engine = Engine::load(&bytes, Tier::all().next().unwrap(), 256, ROOMY).unwrap();
    let reply = ask(
        &mut engine,
        &Asked {
            jump: true,
            ..SESSION[0]
        },
    )
    .unwrap();
    assert!(reply.text().starts_with("{\"script\":\""), "{reply:?}");
    // Without it, every token is the model's pick.
    let mut engine = Engine::load(&bytes, Tier::all().next().unwrap(), 256, ROOMY).unwrap();
    assert_eq!(ask(&mut engine, &SESSION[0]).unwrap().forced, 0);
    assert!(reply.forced > 0, "{reply:?}");
}

/// Checks the cache holds a key and value per token behind it, the ones the model
/// gives those tokens from an empty cache, bit for bit.
pub(crate) fn assert_cache_holds_its_tokens(bytes: &[u8], tier: Tier, engine: &Engine<'_>) {
    let (tokens, kv) = engine.cache();
    assert_eq!(tokens.len(), kv.len());
    let Some(last) = tokens.len().checked_sub(1) else {
        return;
    };
    let file = gguf::Gguf::parse(bytes).unwrap();
    let mut model = crate::Model::new(&file, tier, kv.capacity()).unwrap();
    let mut fresh = model.kv_cache().unwrap();
    for &token in tokens {
        model.forward(&mut fresh, token, None).unwrap();
    }
    let bits = |values: &[u16]| values.to_vec();
    for layer in 0..model.config().n_layer {
        let ((keys, values), (want_keys, want_values)) =
            (kv.layer(layer, last), fresh.layer(layer, last));
        assert_eq!(bits(keys), bits(want_keys), "layer {layer}");
        assert_eq!(bits(values), bits(want_values), "layer {layer}");
    }
}

#[test]
fn the_cache_holds_exactly_what_its_tokens_give() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        for asked in &SESSION {
            let _ = ask(&mut engine, asked);
            assert_cache_holds_its_tokens(&bytes, tier, &engine);
        }
    }
}

#[test]
fn a_stop_in_the_prompt_keeps_what_ran() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let mut warm = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let turns = SESSION[0].turns();
        let mut steps = 0;
        let stopped = warm
            .reply(&turns, 20, &mut Sampler::greedy(), None, |step| {
                steps += matches!(step, Step::Prompt) as usize;
                match steps == 7 {
                    true => Next::Stop,
                    false => Next::Continue,
                }
            })
            .unwrap();
        assert_eq!(stopped.tokens, 0);
        assert_cache_holds_its_tokens(&bytes, tier, &warm);
        // The next request runs on from the whole block that ran, the stop
        // coming after it.
        let next = ask(&mut warm, &SESSION[0]).unwrap();
        assert_eq!(next.cached, crate::PREFILL_TOKENS);
        let mut fresh = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        assert!(same(&Ok(next), &ask(&mut fresh, &SESSION[0])));
        // A stop at the cache's own step keeps the cached prompt too.
        let mut first = true;
        let stopped = warm
            .reply(&turns, 20, &mut Sampler::greedy(), None, |_| {
                let stop = first;
                first = false;
                match stop {
                    true => Next::Stop,
                    false => Next::Continue,
                }
            })
            .unwrap();
        assert_cache_holds_its_tokens(&bytes, tier, &warm);
        let again = ask(&mut warm, &SESSION[0]).unwrap();
        assert_eq!(again.cached, stopped.cached);
        let mut fresh = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        assert!(same(&Ok(again), &ask(&mut fresh, &SESSION[0])));
    }
}

#[test]
fn a_prompt_that_carries_an_answer_runs_on_from_it() {
    // The answer's own positions are reused when the next prompt repeats it,
    // think block and all, as a continuation of the same turn would.
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let mut warm = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let stopped = Asked {
            stop_at: Some(6),
            ..SESSION[0]
        };
        let first = ask(&mut warm, &stopped).unwrap();
        assert_eq!(first.tokens, 6);
        let said = ["<think>\n\n</think>\n\n", first.text().as_str()].concat();
        let mut turns = stopped.turns();
        turns.push(Turn::new(Role::Assistant, &said));
        turns.push(Turn::new(Role::User, "again"));
        let mut sampler = Sampler::seeded(0.2, 42);
        let reply = |engine: &mut Engine<'_>, sampler: &mut Sampler| {
            engine.reply(&turns, 30, sampler, Some(&SCRIPT_SHAPE), |_| Next::Continue)
        };
        let got = reply(&mut warm, &mut sampler).unwrap();
        // All but the last answer token went through the model.
        assert_eq!(got.cached, first.prompt_tokens + first.tokens - 1);
        let mut fresh = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let want = reply(&mut fresh, &mut Sampler::seeded(0.2, 42));
        assert!(same(&Ok(got), &want));
        assert_cache_holds_its_tokens(&bytes, tier, &warm);
    }
}

#[test]
fn truncating_the_cache_keeps_only_its_start() {
    let config = Config::from_gguf(
        gguf::Gguf::parse(&fixture::build(&Spec::tiny()))
            .unwrap()
            .header(),
    )
    .unwrap();
    let mut kv = KvCache::new(&config, 16).unwrap();
    kv.truncate(4);
    assert!(kv.is_empty());
    let mut kv2 = KvCache::new(&config, 16).unwrap();
    for _ in 0..5 {
        kv2.advance(1);
    }
    kv2.truncate(9);
    assert_eq!(kv2.len(), 5);
    kv2.truncate(2);
    assert_eq!(kv2.len(), 2);
    kv2.clear();
    assert_eq!(kv2.len(), 0);
}
