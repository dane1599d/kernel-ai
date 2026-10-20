use super::fake::{Fake, TOKEN, kernel, opened};
use crate::service::Service;

#[test]
fn status_says_what_is_open_and_how_the_last_build_went() {
    let mut service = Service::new("10.0.2.2:5000", TOKEN);
    let mut env = Fake::new("x\n");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok no file is open"
    );
    let (mut service, mut env) = opened("x\n");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok notes 2: unchanged"
    );
    kernel(&mut service, &mut env, "cmd c 1 y");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok notes 2: edited"
    );
    kernel(&mut service, &mut env, "cmd build");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok notes 2: edited, built e1\nlast build: ok built e1 from 2 (commit 1a2b3c4d5e6f); :apply runs it"
    );
}

#[test]
fn the_moved_mark_splits_only_a_well_formed_first_line() {
    use crate::ipc::{split_moved, with_moved};
    let marked = with_moved("components/notes/src/lib.rs:4", "ok p\nmore");
    assert_eq!(
        split_moved(marked.as_bytes()),
        (Some("components/notes/src/lib.rs:4"), &b"ok p\nmore"[..])
    );
    for whole in [
        &b"ok plain"[..],
        b"#at lib.rs:4",
        b"#at \xff\xfe:4\nok",
        b"ok p\n#at lib.rs:4",
    ] {
        assert_eq!(split_moved(whole), (None, whole));
    }
}
