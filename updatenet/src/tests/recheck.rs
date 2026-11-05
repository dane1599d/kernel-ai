//! The signed metadata asked again before the slot becomes a trial, and which
//! TLS stops are the network's.

use super::*;
use crate::stage::Staged;
use core::cell::Cell;
use kainet::tls::TlsError;

type Pair = (TargetFile, TargetFile);

/// One stage of `r` against `server`, with `recheck` in place of the repository.
fn stage_with(
    world: &mut World,
    r: &Release7,
    server: &mut Server,
    recheck: &mut dyn FnMut() -> Result<Pair, Refused>,
) -> Result<Staged, Stop> {
    let mut partial = SlotPartial::new(&mut world.slot, &mut world.memo, &r.job.release);
    stage(
        &r.job,
        &r.list,
        &mut world.nv,
        &mut world.var,
        &mut partial,
        recheck,
        &mut |range, sink| server.fetch(range, sink),
    )
}

/// What the repository signs for `r`'s bytes as release `seq` under `tag`, with
/// the list text `text`.
fn pair(r: &Release7, seq: u64, tag: &str, text: &str) -> Pair {
    (image_target(&r.bytes, seq, tag), list_target(text))
}

#[test]
fn the_metadata_is_asked_once_after_the_last_chunk_and_the_trial_follows_it() {
    let r = release(2, 40);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes);
    let calls = Cell::new(0);
    let mut good = signed(&r);
    let mut counted = || {
        calls.set(calls.get() + 1);
        good()
    };
    let staged = stage_with(&mut world, &r, &mut server, &mut counted).unwrap();
    assert_eq!(staged.release_seq, SEQ);
    assert_eq!(calls.get(), 1);
    assert_eq!(world.var.sets.len(), 2, "Updating, then the trial");
    assert!(bootslot::may_boot(&world.var.sets[1].record, Slot::B));
}

#[test]
fn a_release_the_repository_no_longer_signs_is_not_made_a_trial() {
    let r = release(2, 41);
    let other = release(2, 42);
    let newer = pair(&other, u64::from(SEQ) + 1, "os-8", &other.text);
    let cases: [(&str, Result<Pair, Refused>); 7] = [
        ("a newer release", Ok(newer)),
        (
            "another tag",
            Ok(pair(&r, u64::from(SEQ), "os-7b", &r.text)),
        ),
        (
            "another chunk list",
            Ok(pair(&r, u64::from(SEQ), TAG, &other.text)),
        ),
        (
            "an image that names no release",
            Ok(pair(&r, 0, TAG, &r.text)),
        ),
        ("the image pulled", Err(Refused::NotSigned(plan::TARGET))),
        (
            "the list pulled",
            Err(Refused::NotSigned(plan::CHUNKS_TARGET)),
        ),
        ("a malformed target", Err(Refused::Field("slot.img"))),
    ];
    for (name, answer) in cases {
        let mut world = World::new(&r);
        let mut server = Server::new(&r.bytes);
        let calls = Cell::new(0);
        let mut recheck = || {
            calls.set(calls.get() + 1);
            answer.clone()
        };
        let stop = stage_with(&mut world, &r, &mut server, &mut recheck).unwrap_err();
        assert_eq!(stop, Stop::Bad(Bad::Superseded), "{name}");
        assert_eq!(calls.get(), 1, "{name}");
        // The image is all there and checked, and still not a trial.
        assert_eq!(world.written(&r), &r.bytes[..], "{name}");
        assert_eq!(world.var.sets.len(), 1, "{name}: never a trial");
        assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
        assert!(stop.why().contains("no longer signs"), "{name}");
    }
}

#[test]
fn metadata_that_cannot_be_confirmed_is_a_retry_and_the_next_call_needs_no_chunk() {
    let r = release(2, 43);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes);
    // The repository's timestamp lapsed while the image was coming down.
    let mut lapsed = || {
        Err(Refused::Repository(String::from(
            "expired: timestamp expired",
        )))
    };
    let stop = stage_with(&mut world, &r, &mut server, &mut lapsed).unwrap_err();
    let Stop::Retry { after_s: None, why } = &stop else {
        panic!("{stop:?}");
    };
    assert!(why.contains("could not be confirmed") && why.contains("expired"));
    assert_eq!(world.var.sets.len(), 1, "never a trial");
    assert_eq!(world.memo.mark.map(|m| m.done), Some(3));
    let fetched = server.served.len();
    // It is back: every chunk is already written, so nothing is fetched.
    let staged = stage_with(&mut world, &r, &mut server, &mut signed(&r)).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (3, 0));
    assert_eq!(server.served.len(), fetched, "no range was asked again");
    // Each call marks the slot Updating first; the trial is the last write.
    assert_eq!(world.var.sets.len(), 3);
    assert!(!bootslot::may_boot(&world.var.sets[1].record, Slot::B));
    assert!(bootslot::may_boot(&world.var.sets[2].record, Slot::B));
}

#[test]
fn a_release_replaced_during_a_long_retry_is_caught_when_the_old_one_finishes() {
    let r = release(3, 44);
    let other = release(3, 45);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes).with(Fault::CutOnce(2 * CHUNK as usize + 1));
    let stop = stage_with(&mut world, &r, &mut server, &mut signed(&r)).unwrap_err();
    assert!(matches!(stop, Stop::Retry { .. }), "{stop:?}");
    // While the machine waited, the repository signed a newer release.
    let mut replaced = || Ok(pair(&other, u64::from(SEQ) + 1, "os-8", &other.text));
    let stop = stage_with(&mut world, &r, &mut server, &mut replaced).unwrap_err();
    assert_eq!(stop, Stop::Bad(Bad::Superseded));
    // Two calls, two Updating marks, and no trial among them: the old image never boots.
    assert_eq!(world.var.sets.len(), 2);
    assert!(
        world
            .var
            .sets
            .iter()
            .all(|nv| !bootslot::may_boot(&nv.record, Slot::B))
    );
    // Planning the new one drops the old mark and rewrites the slot.
    let mut fresh = Server::new(&other.bytes);
    let staged = stage_with(&mut world, &other, &mut fresh, &mut signed(&other)).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (0, 4));
}

#[test]
fn the_metadata_is_not_asked_when_the_download_stopped() {
    let r = release(2, 46);
    for fault in [Fault::Flip(CHUNK as usize + 3), Fault::CutOnce(100)] {
        let mut world = World::new(&r);
        let mut server = Server::new(&r.bytes).with(fault.clone());
        let calls = Cell::new(0);
        let mut recheck = || {
            calls.set(calls.get() + 1);
            Err(Refused::NotSigned(plan::TARGET))
        };
        let stop = stage_with(&mut world, &r, &mut server, &mut recheck).unwrap_err();
        assert!(!matches!(stop, Stop::Bad(Bad::Superseded)), "{fault:?}");
        assert_eq!(calls.get(), 0, "{fault:?}");
    }
}

#[test]
fn a_transport_that_failed_under_tls_is_a_retry_and_a_record_it_cannot_take_is_a_refusal() {
    let r = release(1, 47);
    for (error, retry) in [
        (
            HttpError::Tls(TlsError::Transport("connection reset")),
            true,
        ),
        (HttpError::Tls(TlsError::Transport("send failed")), true),
        (HttpError::Tls(TlsError::Truncated), true),
        (HttpError::Tls(TlsError::TooLarge), false),
        (HttpError::TooLarge, false),
    ] {
        let mut world = World::new(&r);
        let server = Server::new(&r.bytes);
        let mut fails =
            |_: Range, _: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| Err(error.clone());
        let mut partial = SlotPartial::new(&mut world.slot, &mut world.memo, &r.job.release);
        let stop = stage(
            &r.job,
            &r.list,
            &mut world.nv,
            &mut world.var,
            &mut partial,
            &mut signed(&r),
            &mut fails,
        )
        .unwrap_err();
        match retry {
            true => assert!(
                matches!(stop, Stop::Retry { after_s: None, .. }),
                "{error:?}: {stop:?}"
            ),
            false => assert!(matches!(stop, Stop::Bad(_)), "{error:?}: {stop:?}"),
        }
        assert!(server.served.is_empty());
    }
}
