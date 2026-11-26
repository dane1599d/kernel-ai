//! A line that `fs` dropped an action of, then went on (bug-hunt-6 H6-24):
//! its next action would begin as the lost one's number, and its commit
//! would make the log call the lost changes landed.

use super::*;
use crate::ledger::Undo;

impl FakeHost {
    /// As the shell's `Files` on `fs`: [`FakeHost::fs_drop`]'s change drops
    /// the open action, its changes gone and its number the next action's,
    /// and a line that lost one begins no other.
    pub(super) fn dropped_or_refused(&self) -> Option<Result<u64, String>> {
        let at = self.fs_drop.get();
        self.fs_drop.set(at.saturating_sub(1));
        let mut ledger = self.ledger.get();
        let out = if at == 1 {
            if let Some(open) = self.open_txn.take() {
                let (before, _) = self.txns.borrow_mut().remove(open);
                *self.files.borrow_mut() = before;
            }
            ledger.dropped_open();
            Some(Err(String::from("Dropped(NoSpace)")))
        } else if self.one_action.get() && self.open_txn.get().is_none() {
            ledger.may_begin().err().map(Err)
        } else {
            None
        };
        self.ledger.set(ledger);
        out
    }

    /// As `Files::undo`: a number the line lost restores nothing, and a
    /// line that lost one restores no other.
    pub(super) fn undo_lost(&self, txn: u64) -> Option<Result<FsUndone, FsRefused>> {
        let ledger = self.ledger.get();
        // Lost, and so never committed here: no transaction holds it.
        let never = self.txns.borrow().len() < txn as usize;
        if never && ledger.open() != Some(txn) && ledger.clone().undo(txn) == Undo::Nothing {
            return Some(Ok(FsUndone {
                generation: txn,
                left: 0,
            }));
        }
        let open = self.open_txn.get().map(|i| i as u64 + 1);
        (open != Some(txn))
            .then(|| ledger.may_begin().map_err(FsRefused::Failed))
            .and_then(Result::err)
            .map(Err)
    }
}

fn script(code: &str) -> Result<llmproto::act::Call, llmproto::act::ActError> {
    let json = format!(r#"{{"name":"script","arguments":{{"code":"{code}"}}}}"#);
    Ok(llmproto::act::parse(&json).unwrap())
}

/// The finding's script, with `b.txt`'s create dropped by `fs`.
fn run(code: &str) -> (Rc<FakeHost>, crate::Reply) {
    let (host, mut shell) = super::act::with_calls(alloc::vec![script(code)]);
    host.one_action.set(true);
    host.files.borrow_mut().insert("a.txt".into(), "0".into());
    host.fs_drop.set(2);
    let reply = shell.eval_console("write a.txt, create b.txt and write c.txt");
    (host, reply)
}

const CAUGHT: &str = "fs_write(\\\"a.txt\\\", \\\"1\\\");\\ntry { fs_create(\\\"b.txt\\\", \\\"x\\\"); } catch {}\\n";

#[test]
fn a_change_after_a_drop_in_the_line_never_lands_the_dropped_ones() {
    let (host, reply) = run(&format!(
        "{CAUGHT}try {{ fs_write(\\\"c.txt\\\", \\\"y\\\"); }} catch {{}}"
    ));
    assert!(!reply.text.contains("a.txt written"), "{}", reply.text);
    assert!(reply.text.contains("did not land"), "{}", reply.text);
    assert_unchanged(&host);
}

#[test]
fn a_run_that_fails_on_the_change_after_a_drop_changed_nothing() {
    let (host, reply) = run(&format!("{CAUGHT}fs_write(\\\"c.txt\\\", \\\"y\\\");"));
    assert!(!reply.text.contains("a.txt written"), "{}", reply.text);
    assert!(reply.text.contains("Nothing changed"), "{}", reply.text);
    assert_unchanged(&host);
}

/// `fs` put a.txt back, and wrote nothing after the drop.
fn assert_unchanged(host: &FakeHost) {
    let files = host.files.borrow();
    assert_eq!(
        files.get("a.txt").map(String::as_str),
        Some("0"),
        "{files:?}"
    );
    assert!(!files.contains_key("c.txt"), "{files:?}");
    assert!(host.txns.borrow().is_empty(), "{:?}", host.txns.borrow());
}

#[test]
fn the_next_line_after_a_drop_changes_files_again() {
    let calls = alloc::vec![
        script(CAUGHT),
        script("fs_write(\\\"c.txt\\\", \\\"y\\\");")
    ];
    let (host, mut shell) = super::act::with_calls(calls);
    host.one_action.set(true);
    host.fs_drop.set(2);
    let lost = shell.eval_console("write a.txt and create b.txt");
    assert!(lost.text.contains("did not land"), "{}", lost.text);
    let next = shell.eval_console("write c.txt");
    assert!(next.ok, "{}", next.text);
    assert_eq!(
        host.files.borrow().get("c.txt").map(String::as_str),
        Some("y")
    );
}
