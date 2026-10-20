use super::*;

#[test]
fn reasons_round_trip_and_have_names() {
    for reason in Reason::ALL {
        assert_eq!(Reason::from_raw(reason as u64), Some(reason));
        assert!(!reason.name().is_empty());
    }
    assert_eq!(Reason::from_raw(0), None);
    assert_eq!(Reason::from_raw(16), None);
}

#[test]
fn labels_are_short_lower_case_words() {
    for good in ["1", "2", "3-trap", "e12", "abcdefghijklmnop"] {
        assert!(valid_label(good), "{good}");
    }
    for bad in ["", "V2", "a b", "../x", "abcdefghijklmnopq", "x_y"] {
        assert!(!valid_label(bad), "{bad}");
    }
}

#[test]
fn only_requests_that_change_nothing_are_relayed() {
    for good in [
        "status",
        "source",
        "source src/lib.rs",
        "try e1",
        " try  3-trap ",
    ] {
        assert!(relayable(good), "{good}");
    }
    for bad in [
        "swap e1",
        "swap e1 force",
        "rollback",
        "reset",
        "try",
        "try ../x",
        "try e1 force",
        "status now",
        "source a b",
        "try\te1\n",
        "",
        "explode",
    ] {
        assert!(!relayable(bad), "{bad:?}");
    }
    let mut long = [b'a'; MAX_CONTROL + 1];
    long[..7].copy_from_slice(b"source ");
    let long = core::str::from_utf8(&long).unwrap();
    assert!(relayable(&long[..MAX_CONTROL]));
    assert!(!relayable(long));
}

#[test]
fn restore_labels_read_back() {
    let mut restore = Restore::empty();
    restore.active[..2].copy_from_slice(b"v2");
    restore.active_len = 2;
    assert_eq!(restore.active(), "v2");
    assert_eq!(restore.candidate(), "");
}

#[test]
fn upload_labels_are_e_and_a_number() {
    for (label, seq) in [("e1", 1), ("e12", 12), ("e0", 0)] {
        assert!(is_upload_label(label), "{label}");
        assert_eq!(upload_seq(label), Some(seq));
    }
    for other in ["e", "1", "ex1", "e-1", "e1a", "3-trap", "E1"] {
        assert!(!is_upload_label(other), "{other}");
    }
}

#[test]
fn commits_are_forty_lowercase_hex_digits() {
    assert!(valid_commit("0123456789abcdef0123456789abcdef01234567"));
    assert!(!valid_commit("0123456789ABCDEF0123456789abcdef01234567"));
    assert!(!valid_commit("0123456789abcdef0123456789abcdef0123456"));
    assert!(!valid_commit("0123456789abcdef0123456789abcdef0123456g"));
}

#[test]
fn info_labels_read_back() {
    let mut info = Info::empty();
    info.next_len = put_label(&mut info.next, "e3");
    info.active_len = put_label(&mut info.active, "2");
    assert_eq!(
        (info.active(), info.next(), info.candidate()),
        ("2", "e3", "")
    );
    let mut long = [0u8; MAX_LABEL];
    assert_eq!(
        put_label(&mut long, "abcdefghijklmnopqrs"),
        MAX_LABEL as u64
    );
}

#[test]
fn publish_is_grantable() {
    assert_ne!(crate::cap::GRANTABLE & crate::cap::PUBLISH, 0);
}
