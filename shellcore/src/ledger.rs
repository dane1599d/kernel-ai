//! The shell's one open file action, and what becomes of a line's when `fs`
//! does not commit it.
//!
//! [`Ledger`] keeps the generation of the line's open action, and of the last
//! one that ended without committing: one `fs` dropped (no room for its
//! writes, a disk error), the shell's own abort (a failed run's rollback), or
//! a Commit `fs` refused. An undo of that one restores nothing. That lasts
//! only until the number commits again: an action that never committed
//! leaves its number to the next commit, whoever makes it.
//!
//! So the log's entries carry it past that. Whatever ends the line's action
//! without committing it (a Commit `fs` refuses, or a write-back `fs` drops
//! on any request, the log's own write included; never the shell's own
//! abort, which logs no file change), the
//! ledger keeps that number as not landed until the line's log is written
//! ([`Ledger::landed`]). That write is then [`LogFailed::NotLanded`], and the
//! shell takes the action's file changes out of its entries
//! ([`take_not_landed`]): nothing of them is undone later, whatever the
//! number has come to mean by then.

use crate::actlog::{self, ActLog, Change, MAX_TEXT};
use crate::undo::Inverse;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// What an `undo` of a generation does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Undo {
    /// The line's own open action: abort it, restore nothing.
    AbortOpen,
    /// An action that never committed: nothing to restore.
    Nothing,
    /// A committed action: `fs` restores it.
    Restore,
}

/// The open action's generation, the last lost one's, and the one the line
/// lost that its log has not yet said so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    open: Option<u64>,
    lost: Option<u64>,
    unlanded: Option<u64>,
}

impl Ledger {
    /// The open action's generation, if any.
    pub fn open(&self) -> Option<u64> {
        self.open
    }

    /// An action began as `generation`: the lost one's number is its now.
    pub fn begin(&mut self, generation: u64) {
        self.open = Some(generation);
        self.lost = None;
    }

    /// `fs` dropped the open action, if there is one.
    pub fn dropped_open(&mut self) {
        if let Some(generation) = self.open.take() {
            self.lose(generation);
        }
    }

    fn lose(&mut self, generation: u64) {
        self.lost = Some(generation);
        self.unlanded = Some(generation);
    }

    /// The open action leaves for its Commit: its generation, if any.
    pub fn take_for_commit(&mut self) -> Option<u64> {
        self.open.take()
    }

    /// The Commit of `generation` was refused: it never committed.
    pub fn commit_refused(&mut self, generation: u64) {
        self.lose(generation);
    }

    /// The shell aborted its open action itself.
    /// Not a loss to say: the shell put the action back itself (a failed
    /// run's rollback, whose entry names no file change), and the log is
    /// still written. Only an undo of it restores nothing.
    pub fn aborted(&mut self, generation: u64) {
        self.open = None;
        self.lost = Some(generation);
    }

    /// `fs` committed something as `generation` (an undo's restore): that
    /// number names a real change now, so its redo restores.
    pub fn committed(&mut self, generation: u64) {
        if self.lost == Some(generation) {
            self.lost = None;
        }
        if self.unlanded == Some(generation) {
            self.unlanded = None;
        }
    }

    /// What the line's log write comes to, given how the line's last Commit
    /// went (`commit`: `Ok` when nothing was open): [`LogFailed::NotLanded`]
    /// for an action the line lost on any request since its last log write,
    /// even one that never reached its Commit, and the loss is then said.
    pub fn landed(&mut self, commit: Result<(), LogFailed>) -> Result<(), LogFailed> {
        let Some(txn) = self.unlanded.take() else {
            return commit;
        };
        match commit {
            Err(LogFailed::NotLanded { txn: refused, why }) if refused == txn => {
                Err(LogFailed::NotLanded { txn, why })
            }
            _ => Err(LogFailed::NotLanded {
                txn,
                why: String::from("fs dropped the action before its Commit"),
            }),
        }
    }

    /// What an undo of `txn` does; an open action it names is aborted.
    pub fn undo(&mut self, txn: u64) -> Undo {
        if self.open == Some(txn) {
            self.aborted(txn);
            Undo::AbortOpen
        } else if self.lost == Some(txn) {
            Undo::Nothing
        } else {
            Undo::Restore
        }
    }
}

/// How `fs` answered the Commit of the line's action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitAnswer {
    /// Committed, as this generation.
    Committed(u64),
    /// Refused, whatever the error: `fs` drops a Commit's changes on any
    /// failure (`Dropped`, `NoSpace`, `ReadOnly`), and before the action is
    /// the caller's there is none of its to land.
    Refused(String),
    /// No answer, or one that is not a Commit's: whether it landed is not known.
    Unknown(String),
}

/// Why the action log was not written ([`crate::Host::log_write`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogFailed {
    /// The Commit of the line's action, `txn`, was refused: neither its file
    /// changes nor the log written with them are on the disk.
    NotLanded { txn: u64, why: String },
    /// The log was not written; the files, if any, are as the line left them.
    Failed(String),
}

impl fmt::Display for LogFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogFailed::NotLanded { why, .. } => write!(f, "the files did not land: {why}"),
            LogFailed::Failed(why) => f.write_str(why),
        }
    }
}

/// What the Commit of action `generation` comes to.
pub fn after_commit(generation: u64, answer: CommitAnswer) -> Result<(), LogFailed> {
    match answer {
        CommitAnswer::Committed(got) if got == generation => Ok(()),
        CommitAnswer::Committed(got) => Err(LogFailed::Failed(format!(
            "the files committed as {got}, not {generation}; undo may miss them"
        ))),
        CommitAnswer::Refused(why) => Err(LogFailed::NotLanded {
            txn: generation,
            why,
        }),
        CommitAnswer::Unknown(why) => Err(LogFailed::Failed(why)),
    }
}

/// Takes `txn`'s file changes out of the log's entries, which say so in
/// their outcome: each such entry's id, with what each change it lost said
/// (its [`Change::what`]). `txn` never committed, so no entry it names
/// landed: the next commit gets its number.
pub fn take_not_landed(log: &mut ActLog, txn: u64, why: &str) -> Vec<(u64, Vec<String>)> {
    let lost = |c: &Change| c.inverse == Some(Inverse::Fs { txn });
    let mut taken = Vec::new();
    for entry in log.entries_mut() {
        if !entry.changes.iter().any(lost) {
            continue;
        }
        let whats = entry.changes.iter().filter(|c| lost(c)).map(Change::what);
        let whats = whats.collect();
        entry.changes.retain(|c| !lost(c));
        let note = format!("; its file changes did not land: {why}");
        entry.outcome = noted(&entry.outcome, &note);
        taken.push((entry.id, whats));
    }
    taken
}

/// `outcome`, then `note`, as one log record carries them: the outcome gives
/// way, so the note is kept whole and an outcome already clipped (a long `c>`
/// entry's) never goes past [`MAX_TEXT`], which would set the whole log aside
/// at the next start.
fn noted(outcome: &str, note: &str) -> String {
    let marked = " ...".len();
    let room = (MAX_TEXT - marked).saturating_sub(note.len() + marked);
    let kept = crate::show::truncate(outcome, room);
    actlog::clip(&format!("{kept}{note}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_is_done_only_as_its_own_generation() {
        assert_eq!(after_commit(7, CommitAnswer::Committed(7)), Ok(()));
        assert!(matches!(
            after_commit(7, CommitAnswer::Committed(8)),
            Err(LogFailed::Failed(_))
        ));
    }

    #[test]
    fn a_dropped_or_no_space_commit_did_not_land() {
        for why in ["Dropped(NoSpace)", "NoSpace", "ReadOnly"] {
            assert_eq!(
                after_commit(7, CommitAnswer::Refused(why.into())),
                Err(LogFailed::NotLanded {
                    txn: 7,
                    why: why.into()
                })
            );
        }
    }

    #[test]
    fn a_commit_with_no_answer_is_not_called_lost() {
        assert!(matches!(
            after_commit(7, CommitAnswer::Unknown("timed out".into())),
            Err(LogFailed::Failed(_))
        ));
    }

    #[test]
    fn a_committed_action_is_restored() {
        let mut ledger = Ledger::default();
        ledger.begin(7);
        assert_eq!(ledger.take_for_commit(), Some(7));
        assert_eq!(ledger.undo(7), Undo::Restore);
    }

    #[test]
    fn a_dropped_open_action_and_a_rolled_back_one_restore_nothing() {
        let mut ledger = Ledger::default();
        ledger.begin(3);
        ledger.dropped_open();
        assert_eq!(ledger.open(), None);
        assert_eq!(ledger.undo(3), Undo::Nothing);
        ledger.begin(4);
        assert_eq!(ledger.undo(4), Undo::AbortOpen);
        assert_eq!(ledger.undo(4), Undo::Nothing);
    }

    #[test]
    fn a_refused_commit_restores_nothing_until_its_number_commits_again() {
        let mut ledger = Ledger::default();
        ledger.begin(7);
        let generation = ledger.take_for_commit().unwrap();
        ledger.commit_refused(generation);
        assert_eq!(ledger.undo(7), Undo::Nothing);
        // An undo's restore commits as 7: its redo is a real change.
        ledger.committed(7);
        assert_eq!(ledger.undo(7), Undo::Restore);
    }

    #[test]
    fn an_action_dropped_at_the_log_write_did_not_land() {
        // fs saves a held write on the log's write, finds no room and drops
        // the action: the Commit is never reached.
        let mut ledger = Ledger::default();
        ledger.begin(7);
        ledger.dropped_open();
        assert_eq!(ledger.take_for_commit(), None);
        assert!(matches!(
            ledger.landed(Ok(())),
            Err(LogFailed::NotLanded { txn: 7, .. })
        ));
        // Said once: the next line's log write is its own.
        assert_eq!(ledger.landed(Ok(())), Ok(()));
    }

    #[test]
    fn an_action_dropped_earlier_in_the_line_did_not_land_though_another_began() {
        let mut ledger = Ledger::default();
        ledger.begin(7);
        ledger.dropped_open();
        // The line's next change begins again, as the same number.
        ledger.begin(7);
        ledger.dropped_open();
        assert!(matches!(
            ledger.landed(Ok(())),
            Err(LogFailed::NotLanded { txn: 7, .. })
        ));
    }

    #[test]
    fn a_refused_commit_did_not_land_with_its_own_reason() {
        let mut ledger = Ledger::default();
        ledger.begin(7);
        let generation = ledger.take_for_commit().unwrap();
        let refused = after_commit(generation, CommitAnswer::Refused("NoSpace".into()));
        ledger.commit_refused(generation);
        assert_eq!(
            ledger.landed(refused),
            Err(LogFailed::NotLanded {
                txn: 7,
                why: "NoSpace".into()
            })
        );
    }

    #[test]
    fn a_lost_number_that_commits_again_in_the_line_landed() {
        // A drop, then the line's next action commits as the same number:
        // that number's changes are on the disk.
        let mut ledger = Ledger::default();
        ledger.begin(7);
        ledger.dropped_open();
        ledger.begin(7);
        let generation = ledger.take_for_commit().unwrap();
        ledger.committed(generation);
        assert_eq!(ledger.landed(Ok(())), Ok(()));
    }

    #[test]
    fn a_rolled_back_action_is_no_loss_and_its_log_is_written() {
        // A run wrote a file, then failed: the shell aborts its action.
        let mut ledger = Ledger::default();
        ledger.begin(7);
        assert_eq!(ledger.undo(7), Undo::AbortOpen);
        assert_eq!(ledger.take_for_commit(), None);
        assert_eq!(ledger.landed(Ok(())), Ok(()));
        // An undo of it still restores nothing.
        assert_eq!(ledger.undo(7), Undo::Nothing);
    }

    #[test]
    fn a_committed_line_landed() {
        let mut ledger = Ledger::default();
        ledger.begin(7);
        let generation = ledger.take_for_commit().unwrap();
        ledger.committed(generation);
        assert_eq!(ledger.landed(Ok(())), Ok(()));
    }

    #[test]
    fn a_new_action_forgets_the_last_loss() {
        let mut ledger = Ledger::default();
        ledger.begin(5);
        ledger.dropped_open();
        ledger.begin(5);
        let generation = ledger.take_for_commit().unwrap();
        assert_eq!(ledger.undo(generation), Undo::Restore);
    }
}
