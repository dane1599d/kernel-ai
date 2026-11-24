//! The shell's one open file action and the one it last lost: which
//! generation an `undo` may restore and which it only forgets.
//!
//! An action `fs` drops (a failed run's rollback, a refused Commit, a full
//! disk) never reaches the disk, and its generation number is the next
//! action's. A restore of it would undo some other action, or none, so it is
//! remembered here and an undo of it restores nothing.

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

/// The open action's generation and the last dropped one's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    open: Option<u64>,
    dropped: Option<u64>,
}

impl Ledger {
    /// The open action's generation, if any.
    pub fn open(&self) -> Option<u64> {
        self.open
    }

    /// An action began as `generation`.
    pub fn begin(&mut self, generation: u64) {
        self.open = Some(generation);
        self.dropped = None;
    }

    /// `fs` dropped the open action, if there is one.
    pub fn dropped_open(&mut self) {
        if let Some(generation) = self.open.take() {
            self.dropped = Some(generation);
        }
    }

    /// The open action leaves for its Commit: its generation, if any.
    pub fn take_for_commit(&mut self) -> Option<u64> {
        self.open.take()
    }

    /// The Commit of `generation` was refused because `fs` had dropped the
    /// action: it never committed, so an undo of it restores nothing.
    pub fn commit_dropped(&mut self, generation: u64) {
        self.dropped = Some(generation);
    }

    /// The open action was aborted by the shell itself.
    pub fn aborted(&mut self, generation: u64) {
        self.open = None;
        self.dropped = Some(generation);
    }

    /// What an undo of `txn` does; an open action it names is aborted.
    pub fn undo(&mut self, txn: u64) -> Undo {
        if self.open == Some(txn) {
            self.aborted(txn);
            Undo::AbortOpen
        } else if self.dropped == Some(txn) {
            Undo::Nothing
        } else {
            Undo::Restore
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undo_of_a_refused_commit_restores_nothing() {
        let mut ledger = Ledger::default();
        ledger.begin(7);
        let generation = ledger.take_for_commit().unwrap();
        ledger.commit_dropped(generation);
        assert_eq!(ledger.undo(7), Undo::Nothing);
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
    fn a_new_action_forgets_the_last_drop() {
        let mut ledger = Ledger::default();
        ledger.begin(5);
        ledger.dropped_open();
        ledger.begin(5);
        let generation = ledger.take_for_commit().unwrap();
        assert_eq!(ledger.undo(generation), Undo::Restore);
    }
}
