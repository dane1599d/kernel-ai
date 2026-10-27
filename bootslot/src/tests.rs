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
fn a_committed_slot_gets_safe_mode_then_the_other_slot() {
    let mut nv = installed();
    step(&mut nv, Event::Update);
    step(&mut nv, Event::UpCommit);
    assert!(nv.record.slot(Slot::B).successful);
    // B now fails every boot before coming up.
    let mut landed = alloc_free_vec();
    for _ in 0..8 {
        landed.push(step(&mut nv, Event::Lost));
    }
    // R counted B's failures: 1 re-arms, 2 re-arms in safe mode, 3 moves to A.
    assert!(landed.contains(&Slot::A), "{landed:?}");
    assert_eq!(nv.record.slot(Slot::B).failures, MAX_FAILURES);
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
fn the_drill_installs_updates_and_recovers_through_r() {
    use crate::drill::{self, Command, End};
    let mut nv = Nv::default();
    drill::install(&mut nv);
    assert_eq!(boot(&mut nv), Slot::A);
    // A committed boot restarts into itself; an update arms B on trial.
    assert_eq!(drill::run(&mut nv, Slot::A, Command::Reboot), End::Reset);
    assert_eq!(nv.one_shot, Some(Slot::A));
    assert_eq!(boot(&mut nv), Slot::A);
    assert_eq!(drill::run(&mut nv, Slot::A, Command::Stage), End::Reset);
    assert_eq!(boot(&mut nv), Slot::B);
    // A panic in the trial lands in R, which re-arms B with a try spent.
    let end = drill::run(&mut nv, Slot::B, Command::PanicLate);
    assert_eq!(end, End::Fault(drill::PANIC));
    fault(&mut nv, Slot::B, drill::PANIC, 0);
    assert_eq!(boot(&mut nv), Slot::R);
    assert_eq!(drill::run(&mut nv, Slot::R, Command::None), End::Reset);
    assert_eq!(boot(&mut nv), Slot::B);
    assert_eq!(drill::run(&mut nv, Slot::B, Command::Commit), End::Reset);
    assert!(nv.record.slot(Slot::B).successful);
    // Recovery on request keeps R up until it is told where to boot.
    assert_eq!(boot(&mut nv), Slot::B);
    assert_eq!(
        drill::run(&mut nv, Slot::B, Command::SetDefault),
        End::SetDefault
    );
    assert_eq!(boot(&mut nv), Slot::R);
    assert_eq!(drill::run(&mut nv, Slot::R, Command::None), End::Stay);
    assert_eq!(drill::stay(&mut nv, Command::Boot(Slot::A)), End::Reset);
    assert_eq!(boot(&mut nv), Slot::A);
    assert!(drill::same(&nv.record, &{
        let mut copy = nv.record;
        copy.boot_seq += 7;
        copy
    }));
}
