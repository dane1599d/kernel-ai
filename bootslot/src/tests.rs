use super::*;

/// What happens during one boot, after the firmware picked the slot.
#[derive(Clone, Copy, Debug)]
enum Event {
    /// Checks pass.
    Up,
    /// Checks pass, then probation.
    UpCommit,
    /// A panic, a failed check, a service given up.
    Fault,
    /// Power lost, or a hang the watchdog resets: nothing written.
    Lost,
    /// From a committed slot: stage the other one and restart into it.
    Update,
    Rollback,
    Recovery,
}

const EVENTS: [Event; 7] = [
    Event::Up,
    Event::UpCommit,
    Event::Fault,
    Event::Lost,
    Event::Update,
    Event::Rollback,
    Event::Recovery,
];

/// A machine with A committed and armed, as an install leaves it.
fn installed() -> Nv {
    let mut nv = Nv::default();
    stage_end(&mut nv, Slot::A, 1);
    commit(&mut nv, Slot::A, 1);
    nv
}

/// One boot: the firmware picks, R decides if it is R, then `event`.
fn step(nv: &mut Nv, event: Event) -> Slot {
    let slot = boot(nv);
    if slot == Slot::R {
        r_boots(nv);
    }
    match event {
        Event::Up => up(nv, slot),
        Event::UpCommit => {
            up(nv, slot);
            commit(nv, slot, 1);
        }
        Event::Fault => fault(nv, slot, 1, 0),
        Event::Lost => {}
        Event::Update => {
            if let Some(other) = slot.other().filter(|_| nv.record.slot(slot).successful) {
                stage_begin(nv, other);
                let seq = nv.record.slot(slot).release_seq + 1;
                stage_end(nv, other, seq);
                restart_into(nv, other);
            }
        }
        Event::Rollback => {
            rollback(nv, slot);
        }
        Event::Recovery => recovery(nv),
    }
    slot
}

/// Every sequence of up to `depth` more events from `nv`: each boot keeps the
/// rule, and so does the boot after the last event. How many sequences ran.
fn explore(nv: Nv, depth: usize, path: &mut [Event; 8], len: usize) -> u64 {
    let next = nv.one_shot.unwrap_or(Slot::R);
    assert!(
        may_boot(&nv.record, next),
        "after {:?} the next boot is {next:?} with {:?}",
        &path[..len],
        nv.record.slot(next)
    );
    if depth == 0 {
        return 1;
    }
    let mut runs = 1;
    for event in EVENTS {
        let mut after = nv;
        step(&mut after, event);
        path[len] = event;
        runs += explore(after, depth - 1, path, len + 1);
    }
    runs
}

#[test]
fn every_sequence_of_eight_events_boots_r_or_a_bootable_slot() {
    let mut path = [Event::Lost; 8];
    let runs = explore(installed(), 8, &mut path, 0);
    // 1 + 7 + 7^2 + ... + 7^8 states visited.
    assert_eq!(runs, (0..=8).map(|n| 7u64.pow(n)).sum::<u64>());
    // And from an empty machine: R only.
    explore(Nv::default(), 8, &mut path, 0);
}

#[test]
fn a_trial_slot_that_never_comes_up_goes_back_to_the_committed_one() {
    let mut nv = installed();
    // A comes up, stages B and restarts into it.
    assert_eq!(step(&mut nv, Event::Update), Slot::A);
    // B faults before coming up, every time; each round through R spends a try.
    let mut landed_on_a = false;
    for _ in 0..2 * usize::from(TRIES) + 2 {
        match boot(&mut nv) {
            Slot::A => {
                landed_on_a = true;
                break;
            }
            Slot::B => fault(&mut nv, Slot::B, 1, 0),
            Slot::R => {
                r_boots(&mut nv);
            }
        }
    }
    assert!(landed_on_a, "B kept booting: {:?}", nv.record.slot(Slot::B));
    assert_eq!(nv.record.slot(Slot::B).reason, Reason::Unbootable);
}

#[test]
fn a_committed_slot_gets_safe_mode_then_r_when_the_other_is_below_the_floor() {
    let mut nv = installed();
    step(&mut nv, Event::Update);
    step(&mut nv, Event::UpCommit);
    assert!(nv.record.slot(Slot::B).successful);
    // B's commit of release 2 left A's release 1 below the floor.
    assert_eq!(nv.record.min_release_seq, 2);
    assert_eq!(nv.record.slot(Slot::A).reason, Reason::Unbootable);
    // B now fails every boot before coming up.
    let mut landed = alloc_free_vec();
    for _ in 0..8 {
        landed.push(step(&mut nv, Event::Lost));
    }
    // R counted B's failures: 1 re-arms, 2 re-arms in safe mode, 3 would move
    // to A, which is below the floor: R stays up (ADR 0008 §7).
    assert!(!landed.contains(&Slot::A), "{landed:?}");
    assert_eq!(nv.record.slot(Slot::B).failures, MAX_FAILURES);
    assert_eq!(nv.record.armed, None);
}

/// No install or update leaves two slots committed at one release, so this
/// state is built by hand: it holds that the floor rule alone would not stop
/// such a fall-back, not that a machine ever takes one.
#[test]
fn a_committed_slot_falls_back_to_another_of_the_same_release() {
    let mut nv = installed();
    // B holds the same release, committed too: at the floor, not below it.
    stage_end(&mut nv, Slot::B, 1);
    commit(&mut nv, Slot::B, 1);
    restart_into(&mut nv, Slot::A);
    commit(&mut nv, Slot::A, 1);
    assert!(nv.record.slot(Slot::B).successful);
    let mut landed = alloc_free_vec();
    for _ in 0..8 {
        landed.push(step(&mut nv, Event::Lost));
    }
    assert!(landed.contains(&Slot::B), "{landed:?}");
}

#[test]
fn no_slot_below_the_floors_boots_whatever_the_record_says() {
    let mut nv = installed();
    // A record as older code left it: B committed at release 1, the floor at 2.
    stage_end(&mut nv, Slot::B, 1);
    nv.record.slot_mut(Slot::B).successful = true;
    nv.record.slot_mut(Slot::A).release_seq = 2;
    nv.record.min_release_seq = 2;
    assert!(!may_boot(&nv.record, Slot::B));
    assert!(may_boot(&nv.record, Slot::A));
    assert!(may_boot(&nv.record, Slot::R));
    // Neither a rollback nor R moves to it.
    assert!(!rollback(&mut nv, Slot::A));
    nv.record.armed = Some(Slot::B);
    assert_eq!(r_boots(&mut nv), InR::Arm(Slot::A));
    // A slot being written is left as it is by a commit.
    let mut nv = installed();
    stage_begin(&mut nv, Slot::B);
    nv.record.slot_mut(Slot::A).release_seq = 5;
    commit(&mut nv, Slot::A, 1);
    assert_eq!(nv.record.slot(Slot::B).reason, Reason::Updating);
}

#[test]
fn a_trial_that_faults_after_coming_up_gets_all_its_tries() {
    let mut nv = installed();
    step(&mut nv, Event::Update);
    // B comes up and then faults, every boot.
    let mut boots_of_b = 0;
    for _ in 0..4 * usize::from(TRIES) {
        match boot(&mut nv) {
            Slot::B => {
                boots_of_b += 1;
                up(&mut nv, Slot::B);
                fault(&mut nv, Slot::B, 1, 0);
            }
            Slot::R => {
                r_boots(&mut nv);
            }
            Slot::A => break,
        }
    }
    assert_eq!(boots_of_b, usize::from(TRIES), "one try each boot, not two");
    assert_eq!(nv.record.slot(Slot::B).reason, Reason::Unbootable);
}

#[test]
fn came_up_round_trips_in_a_byte_older_records_leave_clear() {
    let mut record = installed().record;
    record.came_up = true;
    let bytes = record.encode();
    assert_eq!(bytes[51], 1);
    assert_eq!(Record::decode(&bytes), Some(record));
    // A record written before it was kept reads as not come up.
    record.came_up = false;
    assert_eq!(record.encode()[51], 0);
    assert_eq!(
        Record::decode(&record.encode()).map(|r| r.came_up),
        Some(false)
    );
}

/// A tiny fixed-size list, since the crate has no allocator.
fn alloc_free_vec() -> heapless_list::List {
    heapless_list::List::default()
}

mod heapless_list {
    use super::Slot;

    #[derive(Debug, Default)]
    pub struct List {
        items: [Option<Slot>; 16],
        len: usize,
    }

    impl List {
        pub fn push(&mut self, slot: Slot) {
            self.items[self.len] = Some(slot);
            self.len += 1;
        }

        pub fn contains(&self, slot: &Slot) -> bool {
            self.items[..self.len].contains(&Some(*slot))
        }
    }
}

#[test]
fn the_record_round_trips_and_damage_reads_as_none() {
    let mut nv = installed();
    step(&mut nv, Event::Update);
    fault(&mut nv, Slot::B, 3, 0x1234);
    nv.record.safe_once = true;
    let bytes = nv.record.encode();
    assert_eq!(Record::decode(&bytes), Some(nv.record));
    for at in 0..BYTES {
        let mut bad = bytes;
        bad[at] ^= 0x40;
        assert_eq!(
            Record::decode(&bad),
            None,
            "a flipped byte {at} still decoded"
        );
    }
    assert_eq!(Record::decode(&bytes[..63]), None);
}

#[test]
fn recovery_on_request_stays_in_r_once_and_counts_no_failure() {
    let mut nv = installed();
    step(&mut nv, Event::Recovery);
    assert_eq!(boot(&mut nv), Slot::R);
    assert_eq!(r_boots(&mut nv), InR::Stay);
    assert_eq!(nv.record.slot(Slot::A).failures, 0);
    assert!(!nv.record.recovery_once);
}

#[test]
fn crc32_matches_the_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn an_install_arms_a_as_a_trial_and_every_sequence_after_it_keeps_the_rule() {
    let mut nv = Nv::default();
    install(&mut nv, 4);
    assert_eq!(nv.one_shot, Some(Slot::A));
    assert!(!nv.record.slot(Slot::A).successful);
    assert_eq!(nv.record.slot(Slot::A).tries, TRIES);
    assert!(nv.record.slot(Slot::A).priority > nv.record.slot(Slot::B).priority);
    assert_eq!(nv.record.slot(Slot::B).release_seq, 4);
    let mut path = [Event::Up; 8];
    assert!(explore(nv, 6, &mut path, 0) > 0);
}

/// From A committed at release 1: B staged with release 2 and armed.
fn staged() -> Nv {
    let mut nv = installed();
    stage_begin(&mut nv, Slot::B);
    stage_end(&mut nv, Slot::B, 2);
    restart_into(&mut nv, Slot::B);
    nv
}

#[test]
fn a_staged_update_is_waiting_until_it_boots_then_on_trial() {
    let mut nv = staged();
    assert_eq!(trial(&nv.record, Slot::B, 2, Slot::A), Trial::Waiting);
    assert_eq!(step(&mut nv, Event::Up), Slot::B);
    assert_eq!(trial(&nv.record, Slot::B, 2, Slot::B), Trial::OnTrial);
}

#[test]
fn an_update_that_commits_held() {
    let mut nv = staged();
    assert_eq!(step(&mut nv, Event::UpCommit), Slot::B);
    for running in [Slot::A, Slot::B, Slot::R] {
        assert_eq!(trial(&nv.record, Slot::B, 2, running), Trial::Held);
    }
}

#[test]
fn an_update_that_spends_its_tries_failed_and_only_then() {
    let mut nv = staged();
    let mut boots = 0;
    while trial(&nv.record, Slot::B, 2, Slot::A) != Trial::Failed {
        // Until the last try is spent, the trial may still commit.
        assert_eq!(trial(&nv.record, Slot::B, 2, Slot::A), Trial::Waiting);
        step(&mut nv, Event::Fault);
        boots += 1;
        assert!(boots < 20, "B never gave up");
    }
    assert_eq!(nv.record.slot(Slot::B).reason, Reason::Unbootable);
}

#[test]
fn an_update_that_never_finished_staging_or_was_replaced_is_abandoned() {
    let mut nv = installed();
    stage_begin(&mut nv, Slot::B);
    // The last BOOTREC_SET failed: B is still being written.
    assert_eq!(trial(&nv.record, Slot::B, 2, Slot::A), Trial::Abandoned);
    stage_end(&mut nv, Slot::B, 3);
    assert_eq!(trial(&nv.record, Slot::B, 2, Slot::A), Trial::Abandoned);
}

#[test]
fn an_nvram_reinstall_never_reads_as_a_failed_update() {
    let mut nv = staged();
    install(&mut nv, 2);
    assert_eq!(trial(&nv.record, Slot::B, 2, Slot::A), Trial::Waiting);
}

/// `bytes` with `at` set to `value` and the CRC made good again, so only the
/// field is wrong.
fn with_byte(mut bytes: [u8; BYTES], at: usize, value: u8) -> [u8; BYTES] {
    bytes[at] = value;
    let crc = crc32(&bytes[..60]);
    bytes[60..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

#[test]
fn a_record_that_does_not_decode_is_not_a_missing_one() {
    let good = installed().record.encode();
    assert_eq!(read(None), Ok(None));
    assert_eq!(read(Some(&good)), Ok(Some(installed().record)));
    let mut bad_crc = good;
    bad_crc[60] ^= 1;
    for (why, bytes) in [
        ("a bad CRC", bad_crc),
        ("VERSION 1.1", with_byte(good, 5, 1)),
        ("VERSION 2.0", with_byte(good, 4, 2)),
        ("a bad reason", with_byte(good, 6 + 3, 3)),
        ("a bad armed slot", with_byte(good, 33, 7)),
    ] {
        assert_eq!(read(Some(&bytes)), Err(Undecodable), "{why}");
    }
    assert_eq!(read(Some(&good[..63])), Err(Undecodable), "too short");
}

#[test]
fn a_system_slot_with_no_record_writes_none_so_r_installs_next() {
    assert_eq!(start(Slot::A, None), Start::NoRecord);
    assert_eq!(start(Slot::B, None), Start::NoRecord);
    // Nothing written: the next boot, R, still finds none and installs.
    assert_eq!(start(Slot::R, None), Start::Install);
    let record = installed().record;
    assert_eq!(start(Slot::R, Some(record)), Start::RBoots(record));
    assert_eq!(start(Slot::A, Some(record)), Start::Slot(record));
    // Why it must not come up: *up* on an empty record changes it, and the
    // kernel would write a record with no floors, armed nowhere.
    let mut empty = Nv::default();
    up(&mut empty, Slot::A);
    assert_ne!(empty.record, Record::default());
}
