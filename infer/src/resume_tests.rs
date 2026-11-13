//! Carrying an answer on (ADR 0013 section 2): a background agent is paused in the
//! middle of a sentence, and when it is resumed it goes on exactly as if it had never
//! stopped, whatever happened to its slot meanwhile. The fixture model runs every
//! tier; each case compares the pieces of an interrupted answer, joined, with the
//! answer made in one go, token for token.

extern crate std;

use crate::chat::{Engine, Role, Step, Turn};
use crate::fixture::{self, Spec};
use crate::segments::Snapshot;
use crate::segments_tests::{ROOMY, key, saved};
use crate::{Error, Next, Sampler};
use alloc::vec::Vec;
use quant::Tier;

/// The most tokens an answer asks for.
const MAX_NEW: usize = 16;

/// An answer's bytes and the tokens it made.
type Piece = (Vec<u8>, Vec<u32>);

fn load(bytes: &[u8], tier: Tier) -> Engine<'_> {
    Engine::load(bytes, tier, 256, ROOMY).unwrap()
}

/// The whole answer to `turns` in one go.
fn whole(engine: &mut Engine<'_>, turns: &[Turn<'_>], sampler: &mut Sampler) -> Piece {
    let mut tokens = Vec::new();
    let reply = engine
        .reply(turns, MAX_NEW, sampler, None, |step| {
            if let Step::Token(token) = step {
                tokens.push(token);
            }
            Next::Continue
        })
        .unwrap();
    (reply.bytes, tokens)
}

/// The first `k` tokens of the answer, then a stop: the last token is the pending one.
fn first(engine: &mut Engine<'_>, turns: &[Turn<'_>], k: usize, sampler: &mut Sampler) -> Piece {
    let mut tokens = Vec::new();
    let reply = engine
        .reply(turns, MAX_NEW, sampler, None, |step| {
            if let Step::Token(token) = step {
                tokens.push(token);
                if tokens.len() == k {
                    return Next::Stop;
                }
            }
            Next::Continue
        })
        .unwrap();
    (reply.bytes, tokens)
}

/// The rest of the answer, from the pending token.
fn rest(
    engine: &mut Engine<'_>,
    pending: u32,
    left: usize,
    sampler: &mut Sampler,
) -> Result<Piece, Error> {
    let mut tokens = Vec::new();
    let reply = engine.continue_reply(pending, left, sampler, |step| {
        if let Step::Token(token) = step {
            tokens.push(token);
        }
        Next::Continue
    })?;
    Ok((reply.bytes, tokens))
}

fn join(a: &Piece, b: &Piece) -> Piece {
    (
        [a.0.as_slice(), b.0.as_slice()].concat(),
        [a.1.as_slice(), b.1.as_slice()].concat(),
    )
}

/// Where to stop an answer of `len` tokens: the first, the middle and the last but one.
fn splits(len: usize) -> Vec<usize> {
    let mut ks = alloc::vec![1, len / 2, len - 1];
    ks.dedup();
    ks
}

/// Another agent's prompt, which shares nothing with `saved()`.
fn other() -> [Turn<'static>; 1] {
    [Turn::new(Role::User, "something else entirely")]
}

#[test]
fn an_answer_stopped_at_any_token_carries_on_as_if_it_never_stopped() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let full = whole(&mut load(&bytes, tier), &saved(), &mut Sampler::greedy());
        assert!(
            full.1.len() >= 3,
            "{}: {} tokens",
            tier.name(),
            full.1.len()
        );
        for k in 1..full.1.len() {
            let mut engine = load(&bytes, tier);
            let mut sampler = Sampler::greedy();
            let head = first(&mut engine, &saved(), k, &mut sampler);
            let pending = *head.1.last().unwrap();
            let tail = rest(&mut engine, pending, MAX_NEW - k, &mut sampler).unwrap();
            assert_eq!(join(&head, &tail), full, "{} stopped at {k}", tier.name());
        }
    }
}

#[test]
fn an_answer_stopped_over_and_over_one_token_at_a_time_is_the_same() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let full = whole(&mut load(&bytes, tier), &saved(), &mut Sampler::greedy());
    // The scheduler's quantum for decoding is one token.
    let mut engine = load(&bytes, tier);
    let mut sampler = Sampler::greedy();
    let mut got = first(&mut engine, &saved(), 1, &mut sampler);
    while got.1.len() < full.1.len() {
        let pending = *got.1.last().unwrap();
        let one = rest(&mut engine, pending, 1, &mut sampler).unwrap();
        got = join(&got, &one);
    }
    assert_eq!(got, full);
}

#[test]
fn another_agent_running_in_another_slot_in_between_changes_nothing() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let full = whole(&mut load(&bytes, tier), &saved(), &mut Sampler::greedy());
        for k in splits(full.1.len()) {
            let mut engine = load(&bytes, tier);
            let theirs = engine.add_slot().unwrap();
            let mut sampler = Sampler::greedy();
            let head = first(&mut engine, &saved(), k, &mut sampler);
            // The foreground takes the model: its own slot, its own prompt, and the
            // engine's logits are its own when it is done.
            engine.use_slot(theirs);
            let user = whole(&mut engine, &other(), &mut Sampler::greedy());
            assert!(!user.1.is_empty());
            engine.use_slot(0);
            let pending = *head.1.last().unwrap();
            let tail = rest(&mut engine, pending, MAX_NEW - k, &mut sampler).unwrap();
            assert_eq!(join(&head, &tail), full, "{} stopped at {k}", tier.name());
        }
    }
}

#[test]
fn a_snapshot_restored_after_another_prompt_took_the_slot_carries_on_exactly() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let full = whole(&mut load(&bytes, tier), &saved(), &mut Sampler::greedy());
        for k in splits(full.1.len()) {
            let mut engine = load(&bytes, tier);
            let mut sampler = Sampler::greedy();
            let head = first(&mut engine, &saved(), k, &mut sampler);
            // Preempted: its state goes to the RAM pool, and another prompt takes the
            // one slot there is.
            let mut snapshot = Snapshot::new();
            engine.snapshot_slot(&key(tier), &mut snapshot).unwrap();
            let user = whole(&mut engine, &other(), &mut Sampler::greedy());
            assert!(!user.1.is_empty());
            // Resumed: the state comes back, and the answer goes on.
            engine
                .restore_snapshot(&key(tier), &saved(), &snapshot)
                .unwrap();
            let pending = *head.1.last().unwrap();
            let tail = rest(&mut engine, pending, MAX_NEW - k, &mut sampler).unwrap();
            assert_eq!(join(&head, &tail), full, "{} stopped at {k}", tier.name());
        }
    }
}

#[test]
fn a_spilled_slot_restored_from_its_file_carries_on_exactly() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let full = whole(&mut load(&bytes, tier), &saved(), &mut Sampler::greedy());
    let k = splits(full.1.len())[1];
    let mut engine = load(&bytes, tier);
    let mut sampler = Sampler::greedy();
    let head = first(&mut engine, &saved(), k, &mut sampler);
    let mut file = Vec::new();
    engine
        .save_slot(&key(tier), &mut |part| {
            file.extend_from_slice(part);
            true
        })
        .unwrap();
    whole(&mut engine, &other(), &mut Sampler::greedy());
    let mut at = 0;
    engine
        .restore_slot(&key(tier), &saved(), &mut |out| {
            let end = at + out.len();
            match file.get(at..end) {
                Some(part) => {
                    out.copy_from_slice(part);
                    at = end;
                    true
                }
                None => false,
            }
        })
        .unwrap();
    let pending = *head.1.last().unwrap();
    let tail = rest(&mut engine, pending, MAX_NEW - k, &mut sampler).unwrap();
    assert_eq!(join(&head, &tail), full);
}

#[test]
fn a_seeded_sampler_goes_on_with_its_draws() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let seeded = || Sampler::seeded(0.9, 4242);
    let full = whole(&mut load(&bytes, tier), &saved(), &mut seeded());
    for k in splits(full.1.len()) {
        let mut engine = load(&bytes, tier);
        let mut sampler = seeded();
        let head = first(&mut engine, &saved(), k, &mut sampler);
        // The same sampler object goes on: its state is where the draws left it.
        let pending = *head.1.last().unwrap();
        let tail = rest(&mut engine, pending, MAX_NEW - k, &mut sampler).unwrap();
        assert_eq!(join(&head, &tail), full, "stopped at {k}");
    }
}

#[test]
fn a_reply_stopped_in_its_prompt_goes_on_by_asking_for_the_same_reply_again() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let full = whole(&mut load(&bytes, tier), &saved(), &mut Sampler::greedy());
    for stop_at in [1, 3, 7] {
        let mut engine = load(&bytes, tier);
        let mut seen = 0;
        let stopped = engine
            .reply(&saved(), MAX_NEW, &mut Sampler::greedy(), None, |step| {
                seen += usize::from(step == Step::Prompt);
                match seen == stop_at {
                    true => Next::Stop,
                    false => Next::Continue,
                }
            })
            .unwrap();
        assert!(stopped.bytes.is_empty(), "stopped in the prompt");
        // The cache holds what ran, and the same reply is the rest of it.
        let again = whole(&mut engine, &saved(), &mut Sampler::greedy());
        assert_eq!(again, full, "stopped at prompt token {stop_at}");
    }
}

#[test]
fn nothing_to_carry_on_and_a_token_that_does_not_exist_are_refused_and_change_nothing() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut empty = load(&bytes, tier);
    let refused = rest(&mut empty, 5, 4, &mut Sampler::greedy());
    assert_eq!(refused, Err(Error::EmptyPrompt));
    let full = whole(&mut load(&bytes, tier), &saved(), &mut Sampler::greedy());
    let mut engine = load(&bytes, tier);
    let mut sampler = Sampler::greedy();
    let head = first(&mut engine, &saved(), 3, &mut sampler);
    let missing = rest(&mut engine, u32::MAX, 4, &mut sampler);
    assert_eq!(missing, Err(Error::Token(u32::MAX)));
    // The refusal left the slot as it was: the right token still carries on.
    let pending = *head.1.last().unwrap();
    let tail = rest(&mut engine, pending, MAX_NEW - 3, &mut sampler).unwrap();
    assert_eq!(join(&head, &tail), full);
}
