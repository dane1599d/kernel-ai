//! `kai-kv 1` files: a restored slot answers as a freshly computed one on every
//! tier, and every mismatch leaves the slot empty for a cold prefill.

extern crate std;

use crate::cache_tests::assert_cache_holds_its_tokens;
use crate::chat::{Budget, Engine, Role, Turn};
use crate::fixture::{self, Spec};
use crate::segments::{Key, Refused};
use crate::{Next, Sampler};
use alloc::vec::Vec;
use quant::Tier;

const ROOMY: Budget = Budget {
    kv_bytes: 64 << 20,
    other_bytes: 64 << 20,
};

fn key(tier: Tier) -> Key {
    Key {
        model_sha256: [7; 32],
        tier,
        pack_id: 0x5cb3_671b_050b_730e,
        slot: 1,
    }
}

/// The prompt a slot is saved for, then one that starts with it.
fn saved() -> [Turn<'static>; 2] {
    [
        Turn::new(Role::System, "You run inside Kernel AI. Answer briefly."),
        Turn::new(Role::User, "hello there"),
    ]
}

fn later() -> [Turn<'static>; 4] {
    [
        Turn::new(Role::System, "You run inside Kernel AI. Answer briefly."),
        Turn::new(Role::User, "hello there"),
        Turn::new(Role::Assistant, "Hi."),
        Turn::new(Role::User, "and now?"),
    ]
}

/// A reply to `turns`, greedy, of at most `max_new` tokens.
fn reply(engine: &mut Engine<'_>, turns: &[Turn<'_>], max_new: usize) -> crate::chat::Reply {
    engine
        .reply(turns, max_new, &mut Sampler::greedy(), None, |_| {
            Next::Continue
        })
        .unwrap()
}

/// `turns`' prompt computed into `engine`, and its slot saved.
fn file(engine: &mut Engine<'_>, key: &Key) -> Vec<u8> {
    // One token made and never run: the cache holds the prompt alone.
    reply(engine, &saved(), 1);
    let mut out = Vec::new();
    engine
        .save_slot(key, &mut |bytes| {
            out.extend_from_slice(bytes);
            true
        })
        .unwrap();
    out
}

fn reader(bytes: &[u8]) -> impl FnMut(&mut [u8]) -> bool + '_ {
    let mut at = 0;
    move |buf| match bytes.get(at..at + buf.len()) {
        Some(part) => {
            buf.copy_from_slice(part);
            at += buf.len();
            true
        }
        None => false,
    }
}

#[test]
fn a_restored_slot_answers_as_a_fresh_one_on_every_tier() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let mut first = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let saved = file(&mut first, &key(tier));
        let mut restored = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let n = restored
            .restore_slot(&key(tier), &later(), &mut reader(&saved))
            .unwrap();
        assert!(n > 0);
        assert_cache_holds_its_tokens(&bytes, tier, &restored);
        let mut fresh = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let (got, want) = (
            reply(&mut restored, &later(), 12),
            reply(&mut fresh, &later(), 12),
        );
        assert_eq!(got.cached, n, "{}", tier.name());
        assert_eq!(want.cached, 0);
        assert_eq!(
            (got.bytes, got.tokens),
            (want.bytes, want.tokens),
            "{}",
            tier.name()
        );
        assert_cache_holds_its_tokens(&bytes, tier, &restored);
    }
}

/// What a refusal case changes: its name, key, file, prompt and the refusal.
type Case<'a> = (&'a str, Key, Vec<u8>, &'a [Turn<'a>], Refused);

#[test]
fn every_mismatch_leaves_the_slot_empty() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut first = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let good = file(&mut first, &key(tier));
    let other = |edit: fn(&mut Key)| {
        let mut k = key(tier);
        edit(&mut k);
        k
    };
    let byte_flipped = |at: usize| {
        let mut b = good.clone();
        b[at] ^= 1;
        b
    };
    let header = crate::segments::HEADER;
    let next = later();
    let cases: Vec<Case<'_>> = alloc::vec![
        (
            "model",
            other(|k| k.model_sha256[0] = 8),
            good.clone(),
            &next,
            Refused::Key
        ),
        (
            "pack",
            other(|k| k.pack_id += 1),
            good.clone(),
            &next,
            Refused::Key
        ),
        (
            "slot",
            other(|k| k.slot = 0),
            good.clone(),
            &next,
            Refused::Key
        ),
        ("magic", key(tier), byte_flipped(0), &next, Refused::Magic),
        (
            "a token",
            key(tier),
            byte_flipped(header),
            &next,
            Refused::Tokens
        ),
        (
            "a value",
            key(tier),
            byte_flipped(good.len() - 1),
            &next,
            Refused::Values
        ),
        (
            "short",
            key(tier),
            good[..good.len() - 2].to_vec(),
            &next,
            Refused::Short
        ),
    ];
    for (what, key, file, turns, why) in cases {
        let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        // Something cached before, which the refusal must not leave behind.
        reply(&mut engine, &[Turn::new(Role::User, "warm")], 1);
        let refused = engine.restore_slot(&key, turns, &mut reader(&file));
        assert_eq!(refused, Err(why), "{what}");
        assert_eq!(reply(&mut engine, &later(), 1).cached, 0, "{what}");
    }
    // Another tier's file is refused too.
    if let Some(other_tier) = Tier::all().find(|&t| t != tier) {
        let mut engine = Engine::load(&bytes, other_tier, 256, ROOMY).unwrap();
        let refused = engine.restore_slot(&key(other_tier), &later(), &mut reader(&good));
        assert_eq!(refused, Err(Refused::Key));
    }
    // A model of another shape.
    let wide = fixture::build(&Spec {
        n_layer: Spec::tiny().n_layer + 1,
        ..Spec::tiny()
    });
    let mut engine = Engine::load(&wide, tier, 256, ROOMY).unwrap();
    let refused = engine.restore_slot(&key(tier), &later(), &mut reader(&good));
    assert!(
        matches!(refused, Err(Refused::Shape | Refused::Tokens)),
        "{refused:?}"
    );
}

#[test]
fn a_stopped_writer_leaves_a_file_that_is_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    reply(&mut engine, &saved(), 1);
    let mut out = Vec::new();
    let mut calls = 0;
    let stopped = engine.save_slot(&key(tier), &mut |part| {
        calls += 1;
        out.extend_from_slice(part);
        calls < 2
    });
    assert!(stopped.is_err());
    let mut fresh = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    assert!(
        fresh
            .restore_slot(&key(tier), &later(), &mut reader(&out))
            .is_err()
    );
}

/// A prompt that shares only the template's start with the file reuses only
/// that, and answers as a fresh engine; the saved prompt itself reuses all but
/// its last token, whose logits the reply needs.
#[test]
fn a_prompt_reuses_only_what_it_shares_with_the_file() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut first = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let saved_file = file(&mut first, &key(tier));
    let bye = [Turn::new(Role::User, "bye")];
    for turns in [&bye[..], &saved()[..]] {
        let mut restored = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let n = restored
            .restore_slot(&key(tier), turns, &mut reader(&saved_file))
            .unwrap();
        let mut fresh = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let (got, want) = (reply(&mut restored, turns, 8), reply(&mut fresh, turns, 8));
        assert_eq!(got.cached, n);
        assert_eq!(n, got.prompt_tokens.min(n));
        assert_eq!((got.bytes, got.tokens), (want.bytes, want.tokens));
    }
}
