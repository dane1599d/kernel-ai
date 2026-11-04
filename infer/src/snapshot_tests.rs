//! RAM snapshots of a KV slot (ADR 0013 section 2, S13-1): a preempted agent's state
//! comes back bit for bit, a snapshot is refused for what it was not saved for, and
//! it is the `kai-kv 1` file without the file.

extern crate std;

use crate::cache_tests::assert_cache_holds_its_tokens;
use crate::chat::{Engine, Role, Turn};
use crate::fixture::{self, Spec};
use crate::segments::{Key, Refused, Snapshot, spill_pack_id};
use crate::segments_tests::{ROOMY, file, key, later, reader, reply, saved};
use alloc::vec::Vec;
use quant::Tier;

/// `engine`'s slot as a snapshot, after the prompt of [`saved`] is computed in it.
fn snap(engine: &mut Engine<'_>, key: &Key) -> Snapshot {
    reply(engine, &saved(), 1);
    let mut snapshot = Snapshot::new();
    engine.snapshot_slot(key, &mut snapshot).unwrap();
    snapshot
}

#[test]
fn a_restored_snapshot_answers_as_a_fresh_slot_on_every_tier() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let mut first = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let snapshot = snap(&mut first, &key(tier));
        assert!(snapshot.positions() > 0 && snapshot.bytes() > 0);
        // The agent was preempted: another prompt takes the slot over.
        reply(&mut first, &[Turn::new(Role::User, "something else")], 4);
        let n = first
            .restore_snapshot(&key(tier), &later(), &snapshot)
            .unwrap();
        assert!(n > 0, "{}", tier.name());
        assert_cache_holds_its_tokens(&bytes, tier, &first);
        let mut fresh = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let (got, want) = (
            reply(&mut first, &later(), 12),
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
        assert_cache_holds_its_tokens(&bytes, tier, &first);
    }
}

#[test]
fn a_snapshot_and_a_file_restore_the_same_cache() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let snapshot = snap(&mut engine, &key(tier));
    let mut out = Vec::new();
    engine
        .save_slot(&key(tier), &mut |b| {
            out.extend_from_slice(b);
            true
        })
        .unwrap();
    let mut from_snapshot = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let mut from_file = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let a = from_snapshot
        .restore_snapshot(&key(tier), &later(), &snapshot)
        .unwrap();
    let b = from_file
        .restore_slot(&key(tier), &later(), &mut reader(&out))
        .unwrap();
    assert_eq!(a, b);
    let ((tokens_a, kv_a), (tokens_b, kv_b)) = (from_snapshot.cache(), from_file.cache());
    assert_eq!(tokens_a, tokens_b);
    assert_eq!(kv_a.len(), kv_b.len());
    let last = kv_a.len() - 1;
    for layer in 0..crate::Config::from_gguf(gguf::Gguf::parse(&bytes).unwrap().header())
        .unwrap()
        .n_layer
    {
        assert_eq!(
            kv_a.layer(layer, last),
            kv_b.layer(layer, last),
            "layer {layer}"
        );
    }
}

#[test]
fn every_mismatch_leaves_the_slot_empty() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut first = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let good = snap(&mut first, &key(tier));
    let other = |edit: fn(&mut Key)| {
        let mut k = key(tier);
        edit(&mut k);
        k
    };
    let next = later();
    let empty = Snapshot::new();
    let cases: [(&str, Key, &Snapshot, &[Turn<'_>], Refused); 4] = [
        (
            "model",
            other(|k| k.model_sha256[0] = 8),
            &good,
            &next,
            Refused::Key,
        ),
        (
            "pack",
            other(|k| k.pack_id += 1),
            &good,
            &next,
            Refused::Key,
        ),
        ("slot", other(|k| k.slot = 0), &good, &next, Refused::Key),
        ("empty", key(tier), &empty, &next, Refused::Key),
    ];
    for (what, key, snapshot, turns, why) in cases {
        let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        // Something cached before, which a refusal must not leave behind.
        reply(&mut engine, &[Turn::new(Role::User, "warm")], 1);
        assert_eq!(
            engine.restore_snapshot(&key, turns, snapshot),
            Err(why),
            "{what}"
        );
        assert_eq!(reply(&mut engine, &later(), 1).cached, 0, "{what}");
    }
    // A model of another shape.
    let wide = fixture::build(&Spec {
        n_layer: Spec::tiny().n_layer + 1,
        ..Spec::tiny()
    });
    let mut engine = Engine::load(&wide, tier, 256, ROOMY).unwrap();
    assert_eq!(
        engine.restore_snapshot(&key(tier), &later(), &good),
        Err(Refused::Shape)
    );
}

#[test]
fn a_snapshot_keeps_its_buffers_when_it_is_saved_over_or_cleared() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let mut snapshot = snap(&mut engine, &key(tier));
    let (reserved, held) = (snapshot.capacities(), snapshot.bytes());
    // Saved over by the same state: the same memory, the same bytes.
    engine.snapshot_slot(&key(tier), &mut snapshot).unwrap();
    assert_eq!((snapshot.capacities(), snapshot.bytes()), (reserved, held));
    // Cleared, it holds nothing and is refused, and the memory is still there.
    snapshot.clear();
    assert_eq!((snapshot.positions(), snapshot.bytes()), (0, 0));
    assert_eq!(snapshot.capacities(), reserved);
    assert_eq!(
        engine.restore_snapshot(&key(tier), &later(), &snapshot),
        Err(Refused::Key)
    );
    // A shorter state saved over a longer one holds only its own positions.
    let mut short = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    reply(&mut short, &[Turn::new(Role::User, "hi")], 1);
    let mut small = Snapshot::new();
    short.snapshot_slot(&key(tier), &mut small).unwrap();
    // (The refused restore above emptied the slot, so its state is computed again.)
    reply(&mut engine, &saved(), 1);
    engine.snapshot_slot(&key(tier), &mut snapshot).unwrap();
    assert!(small.positions() < snapshot.positions());
    snapshot = small;
    assert!(snapshot.bytes() < held);
}

#[test]
fn a_reserved_snapshot_serves_every_size_from_one_buffer() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let mut snapshot = Snapshot::new();
    engine.reserve_snapshot(&mut snapshot, 200).unwrap();
    let reserved = snapshot.capacities();
    assert!(reserved.0 > 0 && reserved.1 >= 200 && snapshot.positions() == 0);
    // A short state, then a longer one, saved into it: the memory never moves.
    reply(&mut engine, &[Turn::new(Role::User, "hi")], 1);
    engine.snapshot_slot(&key(tier), &mut snapshot).unwrap();
    let short = snapshot.positions();
    assert_eq!(snapshot.capacities(), reserved);
    reply(&mut engine, &saved(), 1);
    engine.snapshot_slot(&key(tier), &mut snapshot).unwrap();
    assert!(snapshot.positions() > short);
    assert_eq!(snapshot.capacities(), reserved);
    // More than a slot holds is capped at the slot's capacity, and what was held is
    // forgotten.
    engine.reserve_snapshot(&mut snapshot, 10_000).unwrap();
    assert_eq!(snapshot.positions(), 0);
    let tokens = snapshot.capacities().1;
    assert!((256..10_000).contains(&tokens), "{tokens}");
}

#[test]
fn one_agents_state_is_refused_for_another() {
    let a = spill_pack_id(7, 1);
    assert_ne!(a, 0);
    assert_eq!(a, spill_pack_id(7, 1), "the same on every run");
    assert_ne!(a, spill_pack_id(8, 1), "another agent");
    assert_ne!(a, spill_pack_id(7, 2), "another request");
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mine = Key {
        pack_id: a,
        ..key(tier)
    };
    let theirs = Key {
        pack_id: spill_pack_id(8, 1),
        ..key(tier)
    };
    let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    // As a snapshot, and as the spill file it is written to.
    let snapshot = snap(&mut engine, &mine);
    let spill = file(&mut engine, &mine);
    assert!(engine.restore_snapshot(&mine, &later(), &snapshot).is_ok());
    assert_eq!(
        engine.restore_snapshot(&theirs, &later(), &snapshot),
        Err(Refused::Key)
    );
    assert!(
        engine
            .restore_slot(&mine, &later(), &mut reader(&spill))
            .is_ok()
    );
    assert_eq!(
        engine.restore_slot(&theirs, &later(), &mut reader(&spill)),
        Err(Refused::Key)
    );
}

#[test]
fn a_snapshot_whose_tokens_do_not_start_the_prompt_is_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let snapshot = snap(&mut engine, &key(tier));
    let (tokens, _) = engine.cache();
    let file = gguf::Gguf::parse(&bytes).unwrap();
    let mut kv = crate::Model::new(&file, tier, 256)
        .unwrap()
        .kv_cache()
        .unwrap();
    // A prompt that starts with other tokens, and one that is just the first token
    // (which leaves the reply nothing to reuse): neither restores anything.
    let other: Vec<u32> = tokens.iter().map(|t| t + 1).collect();
    for prompt in [other.as_slice(), &tokens[..1]] {
        let refused = crate::segments::restore_snapshot(&key(tier), &mut kv, prompt, &snapshot);
        assert_eq!(refused, Err(Refused::Tokens));
        assert!(kv.is_empty());
    }
    // And the prompt the snapshot was made for restores it all but the last token.
    let again = crate::segments::restore_snapshot(&key(tier), &mut kv, tokens, &snapshot);
    assert_eq!(again.as_deref(), Ok(tokens));
    assert_eq!(kv.len(), tokens.len());
}

/// A spill file in a buffer, for the guest's measurement run on a fixture.
struct Buffer {
    bytes: Vec<u8>,
    /// Whether writes succeed.
    works: bool,
}

impl crate::preempt::Spill for Buffer {
    fn writer(&mut self) -> crate::preempt::Write<'_> {
        self.bytes.clear();
        let works = self.works;
        let bytes = &mut self.bytes;
        alloc::boxed::Box::new(move |part| {
            bytes.extend_from_slice(part);
            works
        })
    }

    fn finish(&mut self) -> bool {
        self.works
    }

    fn reader(&mut self) -> crate::preempt::Read<'_> {
        let (bytes, mut at) = (&self.bytes, 0);
        alloc::boxed::Box::new(move |buf| match bytes.get(at..at + buf.len()) {
            Some(part) => {
                buf.copy_from_slice(part);
                at += buf.len();
                true
            }
            None => false,
        })
    }
}

#[test]
fn the_guests_measurement_finds_a_restored_slot_equal_and_says_what_it_could_not_do() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut engine = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
    let mut spill = Buffer {
        bytes: Vec::new(),
        works: true,
    };
    let now = core::cell::Cell::new(0u64);
    let clock = || {
        now.set(now.get() + 1_000_000);
        now.get()
    };
    let mut told = Vec::new();
    // Sizes that fit the slot (256, less the room the follow-up needs), and one that
    // does not.
    let rows = crate::preempt::measure(
        &mut engine,
        &key(tier),
        &[60, 130, 250],
        &mut spill,
        &clock,
        &|| (),
        &mut |size, row| told.push((size, row.is_ok())),
    );
    assert_eq!(rows.len(), 3);
    // Each size was told as it was done, in order, the last one as not measured.
    assert_eq!(told, [(60, true), (130, true), (250, false)]);
    let (small, large) = (rows[0].as_ref().unwrap(), rows[1].as_ref().unwrap());
    assert!(
        small.positions > 20 && small.positions < large.positions,
        "{small:?} {large:?}"
    );
    assert!(small.equal() && large.equal(), "{small:?} {large:?}");
    // The fake clock moves a millisecond a reading, so every step takes a few.
    assert!(small.restore_ns >= 1_000_000 && small.spill_save_ns >= 1_000_000);
    assert!(
        small
            .line()
            .contains("equal after restore: snapshot true, spill true")
    );
    assert!(rows[2].as_ref().unwrap_err().contains("do not fit"));
    // A spill target that cannot be written says so rather than reporting a row.
    spill.works = false;
    let rows = crate::preempt::measure(
        &mut engine,
        &key(tier),
        &[60],
        &mut spill,
        &clock,
        &|| (),
        &mut |_, _| (),
    );
    assert!(
        rows[0]
            .as_ref()
            .unwrap_err()
            .contains("spill file was not written")
    );
}
