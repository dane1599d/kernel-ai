//! `kai-modeltrial 1` and the probation a switched model is on.

use modelchan::trial::{
    AtBoot, Decision, HUNG, Health, MAX_WHY, State, Switch, Trial, at_boot, decide, switch,
};

const OLD: [u8; 32] = [0x11; 32];
const NEW: [u8; 32] = [0x22; 32];
const OTHER: [u8; 32] = [0x33; 32];

/// A model's FNV-1a64, made up for the test.
fn fnv_of(model: [u8; 32]) -> u64 {
    u64::from(model[0]) * 0x0101_0101_0101_0101
}

fn pending(candidate: [u8; 32], previous: Option<[u8; 32]>) -> Trial {
    Trial {
        candidate,
        fnv: fnv_of(candidate),
        previous,
        state: State::Pending,
    }
}

#[test]
fn a_record_reads_back_as_written() {
    let records = [
        pending(NEW, Some(OLD)),
        pending(NEW, None),
        Trial {
            state: State::Attempted,
            ..pending(NEW, Some(OLD))
        },
        Trial {
            state: State::Kept,
            ..pending(NEW, Some(OLD))
        },
        Trial {
            state: State::RolledBack("it did not load within 300 s".into()),
            ..pending(NEW, Some(OLD))
        },
    ];
    for record in records {
        assert_eq!(Trial::parse(&record.encode()), Ok(record.clone()));
    }
    assert_eq!(
        pending(NEW, None).encode(),
        format!(
            "kai-modeltrial 1\ncandidate {}\nfnv 2222222222222222\nprevious none\nstate pending\n",
            "22".repeat(32)
        )
    );
}

#[test]
fn a_why_is_kept_to_one_short_line() {
    let long = format!("panic\nin {}", "x".repeat(400));
    let record = Trial {
        state: State::RolledBack(long),
        ..pending(NEW, Some(OLD))
    };
    let back = Trial::parse(&record.encode()).unwrap();
    let State::RolledBack(why) = back.state else {
        panic!("{back:?}")
    };
    assert!(
        why.starts_with("panic in xxx") && why.len() <= MAX_WHY,
        "{why}"
    );
    let blank = Trial {
        state: State::RolledBack("\n".into()),
        ..pending(NEW, Some(OLD))
    };
    let back = Trial::parse(&blank.encode()).unwrap();
    assert_eq!(back.state, State::RolledBack("no reason was given".into()));
}

#[test]
fn a_malformed_record_is_refused() {
    let good = pending(NEW, Some(OLD)).encode();
    let rolled = Trial {
        state: State::RolledBack("x".into()),
        ..pending(NEW, Some(OLD))
    }
    .encode();
    let bad = [
        good.trim_end().to_string(),
        good.replace("kai-modeltrial 1", "kai-modeltrial 2"),
        good.replace(
            &"22".repeat(32),
            &"2".repeat(64).to_uppercase().replace('2', "A"),
        ),
        good.replace(&"11".repeat(32), "11"),
        good.replace("state pending", "state kept-ish"),
        good.replace("candidate ", "candidate  "),
        format!("{good}extra\n"),
        good.replace("\nprevious", "\nprevius"),
        good.replace("fnv 2222222222222222", "fnv 222222222222222G"),
        good.replace("fnv 2222222222222222", "fnv 2222222222222222A"),
        good.replace("fnv 2222222222222222", "fnv 22222222"),
        good.replace("\nfnv 2222222222222222", ""),
        rolled.replace("why x", "why "),
        rolled.replace("why x", &format!("why {}", "y".repeat(MAX_WHY + 1))),
        rolled.replace("\nwhy x", ""),
        String::new(),
    ];
    for text in bad {
        assert!(Trial::parse(&text).is_err(), "{text:?}");
    }
}

#[test]
fn a_switch_tries_another_model_and_undo_cancels_it() {
    // From OLD to NEW: a trial, with OLD to go back to.
    let tried = switch(Some(OLD), None, NEW, fnv_of(NEW));
    assert_eq!(tried, Switch::Try(pending(NEW, Some(OLD))));
    let Switch::Try(record) = tried else {
        unreachable!()
    };
    // Again: nothing more to do. Back to OLD before the restart: cancelled.
    assert_eq!(
        switch(Some(OLD), Some(&record), NEW, fnv_of(NEW)),
        Switch::Nothing
    );
    assert_eq!(
        switch(Some(OLD), Some(&record), OLD, fnv_of(OLD)),
        Switch::Cancel
    );
    // A third model replaces the pending one, still going back to OLD.
    assert_eq!(
        switch(Some(OLD), Some(&record), OTHER, fnv_of(OTHER)),
        Switch::Try(pending(OTHER, Some(OLD)))
    );
    // The running model, with nothing pending: nothing.
    assert_eq!(switch(Some(OLD), None, OLD, fnv_of(OLD)), Switch::Nothing);
    let kept = Trial {
        state: State::Kept,
        ..record.clone()
    };
    assert_eq!(
        switch(Some(NEW), Some(&kept), NEW, fnv_of(NEW)),
        Switch::Nothing
    );
    assert_eq!(
        switch(Some(NEW), Some(&kept), OLD, fnv_of(OLD)),
        Switch::Try(pending(OLD, Some(NEW)))
    );
    // No model ran: the trial goes back to none.
    assert_eq!(
        switch(None, None, NEW, fnv_of(NEW)),
        Switch::Try(pending(NEW, None))
    );
}

#[test]
fn the_candidate_is_kept_only_once_it_loads() {
    const FNV: u64 = 0xfeed;
    let limit = 300_000;
    assert_eq!(
        decide(FNV, Health::Ready { fnv: FNV }, 5, limit),
        Decision::Keep
    );
    assert_eq!(decide(FNV, Health::NotYet, 1_000, limit), Decision::Wait);
    let back = |health, waited| match decide(FNV, health, waited, limit) {
        Decision::RollBack(why) => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(back(Health::NotYet, limit), "it did not load within 300 s");
    assert_eq!(
        back(Health::Ready { fnv: 1 }, 5),
        "another model started in its place"
    );
    assert_eq!(back(Health::NoModel, 5), "no model started");
    assert_eq!(
        back(Health::Failed("refused header\nline two"), 5),
        "it did not load: refused header line two"
    );
}

#[test]
fn a_boot_marks_a_trial_attempted_and_one_never_decided_rolls_back() {
    let record = pending(NEW, Some(OLD));
    let attempted = Trial {
        state: State::Attempted,
        ..record.clone()
    };
    assert_eq!(at_boot(Some(&record)), AtBoot::Try(attempted.clone()));
    // That boot hung before it decided: the next one does not try again.
    assert_eq!(
        at_boot(Some(&attempted)),
        AtBoot::RollBack(Trial {
            state: State::RolledBack(HUNG.into()),
            ..record.clone()
        })
    );
    let decided = [
        Trial {
            state: State::Kept,
            ..record.clone()
        },
        Trial {
            state: State::RolledBack("no model started".into()),
            ..record.clone()
        },
    ];
    for t in &decided {
        assert_eq!(at_boot(Some(t)), AtBoot::Nothing);
    }
    assert_eq!(at_boot(None), AtBoot::Nothing);
    // An attempted trial is undecided: replaced or cancelled as a pending one is.
    assert_eq!(
        switch(Some(OLD), Some(&attempted), NEW, fnv_of(NEW)),
        Switch::Nothing
    );
    assert_eq!(
        switch(Some(NEW), Some(&attempted), OLD, fnv_of(OLD)),
        Switch::Try(pending(OLD, Some(NEW)))
    );
    assert_eq!(
        switch(Some(OLD), Some(&attempted), OLD, fnv_of(OLD)),
        Switch::Cancel
    );
    assert_eq!(
        switch(Some(OLD), Some(&attempted), OTHER, fnv_of(OTHER)),
        Switch::Try(pending(OTHER, Some(OLD)))
    );
}
