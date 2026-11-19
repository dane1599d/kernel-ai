//! `models remove`: a model running, on trial or being imported stays; one
//! downloading stops; any other stored one leaves the store.

use modelchan::remove::{Holds, Remove, decide, trial_holds};
use modelchan::trial::{State, Trial};

const A: [u8; 32] = [0xaa; 32];
const B: [u8; 32] = [0xbb; 32];

#[test]
fn a_model_nothing_holds_leaves_the_store() {
    assert_eq!(decide(&A, &Holds::default()), Ok(Remove::Blob));
    let others = Holds {
        running: Some(B),
        candidate: Some(B),
        previous: Some(B),
        importing: Some(B),
        downloading: Some(B),
        busy: false,
    };
    assert_eq!(decide(&A, &others), Ok(Remove::Blob));
}

#[test]
fn a_model_running_on_trial_or_being_imported_stays() {
    for (holds, words) in [
        (
            Holds {
                running: Some(A),
                ..Holds::default()
            },
            "the model running",
        ),
        (
            Holds {
                candidate: Some(A),
                ..Holds::default()
            },
            "the next restart starts it",
        ),
        (
            Holds {
                previous: Some(A),
                ..Holds::default()
            },
            "falls back to",
        ),
        (
            Holds {
                importing: Some(A),
                ..Holds::default()
            },
            "being imported",
        ),
    ] {
        let why = decide(&A, &holds).unwrap_err();
        assert!(why.contains(words), "{why}");
    }
}

/// Fix check of #1161: a blob is not removed while an import or a download
/// of another model holds the store; the download of this one still stops.
#[test]
fn a_busy_store_refuses_a_blob_but_stops_its_own_download() {
    let busy = Holds {
        busy: true,
        ..Holds::default()
    };
    assert!(
        decide(&A, &busy)
            .unwrap_err()
            .contains("try again when it finishes")
    );
    let downloading = Holds {
        downloading: Some(A),
        busy: true,
        ..Holds::default()
    };
    assert_eq!(decide(&A, &downloading), Ok(Remove::Download));
}

#[test]
fn a_download_of_it_stops_instead() {
    let holds = Holds {
        downloading: Some(A),
        ..Holds::default()
    };
    assert_eq!(decide(&A, &holds), Ok(Remove::Download));
    // Running wins over downloading: a model never runs mid-download, but
    // the refusal is the safe answer if both were ever said.
    let both = Holds {
        running: Some(A),
        downloading: Some(A),
        ..Holds::default()
    };
    assert!(decide(&A, &both).is_err());
}

/// A trial holds what the hint names or may go back to: undecided, its
/// candidate and its way back; kept, its candidate; rolled back, its previous,
/// which the next boot runs.
#[test]
fn a_trial_holds_what_the_next_boot_may_map() {
    let trial = |state| Trial {
        candidate: A,
        fnv: 7,
        previous: Some(B),
        state,
    };
    for state in [State::Pending, State::Attempted] {
        assert_eq!(trial_holds(Some(&trial(state))), (Some(A), Some(B)));
    }
    assert_eq!(trial_holds(Some(&trial(State::Kept))), (Some(A), None));
    let rolled = trial(State::RolledBack("no".into()));
    assert_eq!(trial_holds(Some(&rolled)), (None, Some(B)));
    // So the fallback a rolled-back trial runs next cannot be removed.
    let (candidate, previous) = trial_holds(Some(&rolled));
    let holds = Holds {
        candidate,
        previous,
        ..Holds::default()
    };
    assert!(decide(&B, &holds).unwrap_err().contains("falls back to"));
    assert_eq!(trial_holds(None), (None, None));
}
