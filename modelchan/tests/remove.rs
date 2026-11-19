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
            "on trial",
        ),
        (
            Holds {
                previous: Some(A),
                ..Holds::default()
            },
            "goes back to it",
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

/// A trial not yet decided holds its candidate and its way back; a decided
/// one holds neither.
#[test]
fn an_undecided_trial_holds_its_candidate_and_its_way_back() {
    let trial = |state| Trial {
        candidate: A,
        fnv: 7,
        previous: Some(B),
        state,
    };
    for state in [State::Pending, State::Attempted] {
        assert_eq!(trial_holds(Some(&trial(state))), (Some(A), Some(B)));
    }
    for state in [State::Kept, State::RolledBack("no".into())] {
        assert_eq!(trial_holds(Some(&trial(state))), (None, None));
    }
    assert_eq!(trial_holds(None), (None, None));
}
