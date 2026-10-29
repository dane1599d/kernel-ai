//! The action log on disk (P2 task 12): written after a line that changed it,
//! read back at start, so `undo` works across a reboot.

use super::scripts::with_answers;
use super::*;
use alloc::vec;

fn answer(script: &str) -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from("Switches."),
        script: String::from(script),
    })
}

#[test]
fn undo_works_after_a_reboot_from_the_log_on_disk() {
    let (host, mut shell) = with_answers(vec![answer("set_policy(\"rr\");")]);
    assert!(shell.eval("? use round robin").ok);
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    let disk = host.log_disk.borrow().clone().unwrap();
    assert!(disk.contains("inv=spec:"), "{disk}");
    // A new shell on the same disk: the log, and so `undo`, come back.
    let mut after = shell_on(&host);
    assert_eq!(after.eval("undo"), ok("Undone: policy back to interactive"));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    // Its next entry follows the old ones.
    assert!(
        after.eval("log").text.contains("#2"),
        "{}",
        after.eval("log").text
    );
}

#[test]
fn a_line_that_changes_nothing_writes_nothing() {
    let (host, mut shell) = with_answers(vec![answer("set_policy(\"rr\");")]);
    shell.eval("1 + 1");
    assert_eq!(host.log_writes.get(), 0);
    shell.eval("? use round robin");
    let writes = host.log_writes.get();
    assert!(writes >= 1);
    shell.eval("2 + 2");
    assert_eq!(host.log_writes.get(), writes);
}

#[test]
fn a_log_that_is_not_actlog_1_is_set_aside() {
    let host = FakeHost::new();
    *host.log_disk.borrow_mut() = Some(String::from("not a log\n"));
    let mut shell = shell_on(&host);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|l| l.contains("not actlog 1"))
    );
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
}

fn shell_on(host: &Rc<FakeHost>) -> Shell {
    super::shell(host)
}
