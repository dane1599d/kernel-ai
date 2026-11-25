//! The stage event over the in-memory slot and a scripted server: streams, cuts,
//! resumes, bad chunks and refusals.

use super::*;

#[test]
fn a_release_streams_in_chunk_by_chunk_and_becomes_a_trial_only_at_the_end() {
    let r = release(3, 10);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes);
    let staged = world.stage(&r, &mut server).unwrap();
    assert_eq!((staged.slot, staged.release_seq), (Slot::B, SEQ));
    assert_eq!((staged.report.kept, staged.report.fetched), (0, 4));
    assert_eq!(world.written(&r), &r.bytes[..]);
    // The slot was marked Updating, and that was written before the first byte
    // reached it; the trial state is the last write.
    assert_eq!(world.var.sets.len(), 2);
    let first = world.var.sets[0].record.slot(Slot::B);
    assert_eq!((first.reason, first.priority), (Reason::Updating, 0));
    assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
    let last = world.var.sets[1].record.slot(Slot::B);
    assert_eq!(
        (last.reason, last.release_seq, last.tries, last.successful),
        (Reason::None, SEQ, bootslot::TRIES, false)
    );
    assert!(last.priority > world.nv.record.slot(Slot::A).priority);
    assert!(bootslot::may_boot(&world.var.sets[1].record, Slot::B));
    // Every chunk was recorded, and the last mark is the whole.
    assert_eq!(world.memo.mark.map(|m| m.done), Some(4));
    // The active slot's record was never touched.
    assert_eq!(world.nv.record.slot(Slot::A), record(SEQ - 1).slot(Slot::A));
}

#[test]
fn a_cut_stops_with_a_retry_and_the_next_call_resumes_at_the_first_unchecked_chunk() {
    let r = release(3, 11);
    let mut world = World::new(&r);
    let cut = 2 * CHUNK as usize + 100;
    let mut server = Server::new(&r.bytes).with(Fault::CutOnce(cut));
    let stop = world.stage(&r, &mut server).unwrap_err();
    assert!(
        matches!(stop, Stop::Retry { after_s: None, .. }),
        "{stop:?}"
    );
    // Two chunks were checked and written; the slot is still Updating.
    assert_eq!(world.memo.mark.map(|m| m.done), Some(2));
    assert!(!bootslot::may_boot(&world.nv.record, Slot::B));
    assert_eq!(world.var.sets.len(), 1);
    assert_eq!(
        world.var.sets[0].record.slot(Slot::B).reason,
        Reason::Updating
    );
    // Resume: the two kept chunks are read back and checked, two are fetched.
    let staged = world.stage(&r, &mut server).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (2, 2));
    assert_eq!(world.written(&r), &r.bytes[..]);
    // Only the cut chunk and the rest were fetched again.
    let from_chunk_2 = server
        .served
        .iter()
        .filter(|(first, _)| *first >= 2 * CHUNK)
        .count();
    assert_eq!(from_chunk_2, 3, "chunk 2 twice, chunk 3 once");
    assert_eq!(
        server.served.iter().filter(|(f, _)| *f < 2 * CHUNK).count(),
        2
    );
}

#[test]
fn a_resume_after_a_reboot_starts_from_the_mark_with_a_fresh_stage_and_server() {
    let r = release(3, 12);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes).with(Fault::CutOnce(CHUNK as usize + 5));
    assert!(world.stage(&r, &mut server).is_err());
    // A reboot forgets everything but the slot, the mark and the boot variable.
    let mut again = World {
        slot: world.slot,
        memo: world.memo,
        var: Var::default(),
        nv: world.var.sets[0],
    };
    let mut fresh = Server::new(&r.bytes);
    let staged = again.stage(&r, &mut fresh).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (1, 3));
    assert_eq!(again.written(&r), &r.bytes[..]);
}

#[test]
fn a_bad_chunk_is_refused_before_a_byte_of_it_is_written_and_the_slot_stays_unbootable() {
    let r = release(3, 13);
    let mut world = World::new(&r);
    let flip = 2 * CHUNK as usize + 9;
    let mut server = Server::new(&r.bytes).with(Fault::Flip(flip));
    assert_eq!(world.stage(&r, &mut server), Err(Stop::Bad(Bad::Chunk(2))));
    // Chunks 0 and 1 were written; nothing of chunk 2 or after.
    assert!(
        world
            .slot
            .writes
            .iter()
            .all(|(offset, _)| *offset < 2 * CHUNK),
        "{:?}",
        world.slot.writes
    );
    assert_eq!(world.memo.mark.map(|m| m.done), Some(2));
    // The boot variable says Updating and nothing after it: it never boots.
    assert_eq!(world.var.sets.len(), 1);
    assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
    // Asked again with a server that is right, it resumes at chunk 2.
    let mut good = Server::new(&r.bytes);
    let staged = world.stage(&r, &mut good).unwrap();
    assert_eq!(staged.report.kept, 2);
    assert_eq!(world.written(&r), &r.bytes[..]);
}

#[test]
fn a_flipped_byte_in_every_chunk_is_refused_by_that_chunk() {
    let r = release(3, 14);
    for chunk in 0..4 {
        let mut world = World::new(&r);
        let at = chunk * CHUNK as usize + 1;
        let mut server = Server::new(&r.bytes).with(Fault::Flip(at));
        assert_eq!(
            world.stage(&r, &mut server),
            Err(Stop::Bad(Bad::Chunk(chunk))),
            "chunk {chunk}"
        );
        assert_eq!(world.memo.mark.map(|m| m.done).unwrap_or(0), chunk);
        assert_eq!(world.var.sets.len(), 1, "never a trial");
    }
}

#[test]
fn a_chunk_that_comes_short_is_refused_not_resumed() {
    let r = release(2, 15);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes).with(Fault::Short(10));
    let stop = world.stage(&r, &mut server).unwrap_err();
    assert_eq!(stop, Stop::Bad(Bad::Length(0)));
    assert!(world.slot.writes.is_empty());
    assert_eq!(world.var.sets.len(), 1);
}

#[test]
fn a_busy_server_is_a_retry_with_its_wait_and_anything_else_is_a_refusal() {
    let r = release(2, 16);
    for (status, after, want) in [
        (429, Some(30), Some(Some(30))),
        (503, None, Some(None)),
        (500, Some(5), Some(Some(5))),
        (404, None, None),
        (403, None, None),
        (200, None, None),
        (302, None, None),
    ] {
        let mut world = World::new(&r);
        let mut server = Server::new(&r.bytes).with(Fault::Status(status, after));
        let stop = world.stage(&r, &mut server).unwrap_err();
        match want {
            Some(wait) => assert!(
                matches!(&stop, Stop::Retry { after_s, .. } if *after_s == wait),
                "{status}: {stop:?}"
            ),
            None => assert_eq!(stop, Stop::Bad(Bad::Status(status)), "{status}"),
        }
        assert!(world.slot.writes.is_empty(), "{status}");
        assert_eq!(world.var.sets.len(), 1);
    }
}

#[test]
fn a_certificate_or_host_refusal_is_a_refusal_and_a_dropped_connection_is_a_retry() {
    let r = release(1, 17);
    let mut world = World::new(&r);
    let mut refused = |_: Range, _: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        Err(HttpError::Refused(String::from("elsewhere.example")))
    };
    let mut partial = SlotPartial::new(&mut world.slot, &mut world.memo, &r.job.release);
    let stop = stage(
        &r.job,
        &r.list,
        &mut world.nv,
        &mut world.var,
        &mut partial,
        &mut signed(&r),
        &mut refused,
    )
    .unwrap_err();
    assert!(matches!(stop, Stop::Bad(Bad::Fetch(_))), "{stop:?}");
    for error in [
        HttpError::Truncated,
        HttpError::Timeout,
        HttpError::Connect("no route"),
    ] {
        let mut world = World::new(&r);
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
        assert!(matches!(stop, Stop::Retry { .. }), "{error:?}: {stop:?}");
    }
}

#[test]
fn a_list_whose_chunks_do_not_make_the_signed_image_forgets_the_mark() {
    let r = release(2, 18);
    // Every chunk hash right, the whole wrong: a list built wrong, as signed.
    let mut list = r.list.clone();
    list.blob = [9; 32];
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes);
    let mut partial = SlotPartial::new(&mut world.slot, &mut world.memo, &r.job.release);
    let stop = stage(
        &r.job,
        &list,
        &mut world.nv,
        &mut world.var,
        &mut partial,
        &mut signed(&r),
        &mut |range, sink| server.fetch(range, sink),
    )
    .unwrap_err();
    assert_eq!(stop, Stop::Bad(Bad::Whole));
    assert_eq!(world.memo.mark, None, "nothing written under it resumes");
    assert_eq!(world.var.sets.len(), 1, "never a trial");
}

#[test]
fn a_slot_that_reads_back_other_than_it_was_written_is_not_staged() {
    let r = release(2, 19);
    let mut world = World::new(&r);
    // The write of chunk 1 is dropped, as a bad sector drops it, though the slot
    // says it was written and the mark records it.
    world.slot.drop_at = Some(CHUNK);
    let mut server = Server::new(&r.bytes);
    let stop = world.stage(&r, &mut server).unwrap_err();
    assert_eq!(stop, Stop::Bad(Bad::ReadBack));
    assert_eq!(world.var.sets.len(), 1, "never a trial");
    assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
}

#[test]
fn a_chunk_that_rotted_on_disk_between_runs_is_fetched_again() {
    let r = release(3, 20);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes).with(Fault::CutOnce(3 * CHUNK as usize));
    assert!(world.stage(&r, &mut server).is_err());
    assert_eq!(world.memo.mark.map(|m| m.done), Some(3));
    // Chunk 1 rots in the slot while the machine is off.
    world.slot.bytes[CHUNK as usize + 3] ^= 0x40;
    let staged = world.stage(&r, &mut server).unwrap();
    assert_eq!(staged.report.kept, 1, "only chunk 0 is still right");
    assert_eq!(world.written(&r), &r.bytes[..]);
}

#[test]
fn a_mark_for_another_release_starts_the_download_over() {
    let old = release(2, 21);
    let new = release(2, 22);
    assert_ne!(old.job.release.sha256, new.job.release.sha256);
    let mut world = World::new(&old);
    let mut server = Server::new(&old.bytes).with(Fault::CutOnce(CHUNK as usize + 1));
    assert!(world.stage(&old, &mut server).is_err());
    assert_eq!(world.memo.mark.map(|m| m.done), Some(1));
    // A newer release was published: the old mark is dropped, the slot rewritten.
    let mut fresh = Server::new(&new.bytes);
    let staged = world.stage(&new, &mut fresh).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (0, 3));
    assert_eq!(world.written(&new), &new.bytes[..]);
    assert_eq!(
        world.memo.mark.map(|m| m.sha256),
        Some(new.job.release.sha256)
    );
}

#[test]
fn a_boot_variable_that_cannot_be_written_stops_before_the_slot_is_touched() {
    let r = release(1, 23);
    let mut world = World::new(&r);
    world.var.fail_nth = Some(1);
    let mut server = Server::new(&r.bytes);
    let stop = world.stage(&r, &mut server).unwrap_err();
    assert_eq!(stop, Stop::Store("the boot variable cannot be written"));
    assert!(world.slot.writes.is_empty());
    assert!(server.served.is_empty());

    // The last write failing leaves the variable as Updating: no trial.
    let mut world = World::new(&r);
    world.var.fail_nth = Some(2);
    let stop = world.stage(&r, &mut Server::new(&r.bytes)).unwrap_err();
    assert!(matches!(stop, Stop::Store(_)));
    assert_eq!(world.var.sets.len(), 1);
    assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
}

#[test]
fn the_slot_is_flushed_before_a_mark_says_a_chunk_is_in_it() {
    // A slot whose flush fails: the mark must not advance past what it holds.
    struct Unflushable(Mem);
    impl SlotIo for Unflushable {
        fn write(&mut self, o: u64, b: &[u8]) -> Result<(), &'static str> {
            self.0.write(o, b)
        }
        fn read(&mut self, o: u64, b: &mut [u8]) -> Result<(), &'static str> {
            self.0.read(o, b)
        }
        fn flush(&mut self) -> Result<(), &'static str> {
            Err("the slot cannot be flushed")
        }
    }
    let r = release(1, 24);
    let mut slot = Unflushable(Mem::new(r.bytes.len() + 4096));
    let mut memo = Memo::default();
    let mut var = Var::default();
    let mut nv = World::new(&r).nv;
    let mut server = Server::new(&r.bytes);
    let mut partial = SlotPartial::new(&mut slot, &mut memo, &r.job.release);
    let stop = stage(
        &r.job,
        &r.list,
        &mut nv,
        &mut var,
        &mut partial,
        &mut signed(&r),
        &mut |range, sink| server.fetch(range, sink),
    )
    .unwrap_err();
    assert_eq!(stop, Stop::Store("the slot cannot be flushed"));
    assert_eq!(memo.mark, None);
}

#[test]
fn every_stop_has_words() {
    let stops = [
        Stop::Retry {
            after_s: Some(30),
            why: String::from("the server answered 429"),
        },
        Stop::Retry {
            after_s: None,
            why: String::from("Truncated"),
        },
        Stop::Bad(Bad::Chunk(3)),
        Stop::Bad(Bad::Length(1)),
        Stop::Bad(Bad::Whole),
        Stop::Bad(Bad::ReadBack),
        Stop::Bad(Bad::Status(404)),
        Stop::Bad(Bad::Fetch(String::from("Refused"))),
        Stop::Store("the slot cannot be flushed"),
    ];
    for stop in stops {
        let words = stop.why();
        assert!(!words.is_empty(), "{stop:?}");
    }
    assert!(Stop::Bad(Bad::Chunk(3)).why().contains("part 3"));
    assert!(
        Stop::Retry {
            after_s: Some(30),
            why: String::new()
        }
        .why()
        .contains("after 30 s")
    );
    assert_eq!(READ_BACK, "the slot reads back other than the signed image");
    for refused in [
        Refused::NotSigned("slot.img"),
        Refused::Repository(String::from("expired")),
        Refused::Field("x"),
        Refused::Rollback { seq: 1, newest: 2 },
        Refused::Slot("x"),
        Refused::Room { need: 2, room: 1 },
        Refused::Staged(1),
        Refused::List("x"),
    ] {
        assert!(!refused.why().is_empty());
    }
}
