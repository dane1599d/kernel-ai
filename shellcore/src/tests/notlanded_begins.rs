//! `Shell::landing_begins` directly (landing.rs): a new line starts with
//! nothing of the last one's loss but a note another write found, which
//! becomes a notice.

use super::*;
use crate::undo::Inverse;

fn write(path: &str, text: &str) -> Result<llmproto::act::Call, llmproto::act::ActError> {
    let code = format!("fs_write(\\\"{path}\\\", \\\"{text}\\\");");
    let json = format!(r#"{{"name":"script","arguments":{{"code":"{code}"}}}}"#);
    Ok(llmproto::act::parse(&json).unwrap())
}

/// The last line's loss (its result, its note's lines, that it ran) is gone
/// once the next line begins.
#[test]
fn a_line_begins_without_the_last_lines_loss() {
    let (host, mut shell) = super::act::with_calls(alloc::vec![write("a.txt", "one")]);
    host.one_action.set(true);
    host.commit_refused.set(true);
    let lost = shell.eval_console("write a.txt");
    assert!(lost.text.contains("did not land"), "{}", lost.text);
    let note = lost
        .text
        .lines()
        .find(|l| l.contains("did not land"))
        .unwrap();
    assert!(shell.is_not_landed_note(note), "{note}");
    assert!(shell.ran_but_did_not_land());
    shell.landing_begins();
    assert!(!shell.is_not_landed_note(note), "{note}");
    assert!(!shell.ran_but_did_not_land());
    assert_eq!(shell.take_not_landed_line(), None);
}

/// A loss found outside a line (a send's log write) is kept until the next
/// line begins, which makes it a notice and leaves nothing for that line's
/// own reply or a save to report.
#[test]
fn a_loss_another_write_found_becomes_a_notice_when_a_line_begins() {
    let (_host, mut shell) = super::act::with_calls(alloc::vec![write("a.txt", "one")]);
    let ran = shell.eval_console("write a.txt");
    assert!(ran.ok, "{}", ran.text);
    let txn = shell
        .log
        .entries()
        .flat_map(|e| &e.changes)
        .find_map(|c| match c.inverse {
            Some(Inverse::Fs { txn }) => Some(txn),
            _ => None,
        })
        .expect("the line's change is logged");
    shell.not_landed(txn, "disk full");
    assert!(shell.notices.is_empty());
    shell.landing_begins();
    assert_eq!(
        shell.notices,
        ["#1: its file changes did not land (disk full); nothing of them to undo"]
    );
    assert_eq!(shell.landed(), Ok(()));
    let reply = crate::Reply::ok("same");
    assert_eq!(shell.say_not_landed(reply.clone()).text, reply.text);
}
