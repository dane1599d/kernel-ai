//! `kai-actions 1` (P4): the rows come from the loaded artifact, with no call
//! into the app, and a suspended app stays suspended.

use super::{V1, call, host, kernel};
use crate::artifact::append_custom_section;
use crate::rows;
use kabi::live::ACTIONS_SECTION;

#[test]
fn the_rows_are_the_artifacts_section_with_its_version() {
    let mut artifact = V1.artifact();
    append_custom_section(
        &mut artifact,
        ACTIONS_SECTION,
        crate::actions::tests::NOTES.as_bytes(),
    );
    let reply = rows::reply("1", &artifact);
    assert!(reply.starts_with("version 1\nkai-actions 1\n"), "{reply}");
    assert_eq!(
        rows::reply("1", &V1.artifact()),
        "err it declares no actions"
    );
    assert!(rows::reply("1", b"not wasm").starts_with("err "));
}

#[test]
fn a_host_answers_without_waking_its_app() {
    let kernel = kernel(&[V1]);
    let mut host = host(&kernel);
    call(&mut host, "ctl suspend");
    assert_eq!(
        call(&mut host, "kai-actions 1"),
        "err it declares no actions"
    );
    assert!(host.suspended());
    assert!(rows::is_request(b"kai-actions 1\n"));
    assert!(!rows::is_request(b"kai-actions 1 more"));
}
