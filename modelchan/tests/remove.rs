//! `models remove`: a model running, on trial or being imported stays; one
//! downloading stops; any other stored one leaves the store.

use modelchan::remove::{Holds, Remove, decide};

const A: [u8; 32] = [0xaa; 32];
const B: [u8; 32] = [0xbb; 32];

#[test]
fn a_model_nothing_holds_leaves_the_store() {
    assert_eq!(decide(&A, &Holds::default()), Ok(Remove::Blob));
    let others = Holds {
        running: Some(B),
        candidate: Some(B),
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
