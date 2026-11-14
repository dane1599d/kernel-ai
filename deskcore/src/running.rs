//! Which person's line the shell runs now, as `desktop` sees it from the
//! shell's takes and results: a Stop or an Esc interrupts the shell only while
//! that person's own line is the one it runs, never another app's effects or
//! a line still waiting (#892's review).

use crate::effects::STOPS_FROM;
use alloc::vec::Vec;

/// A person's line the shell may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// A line typed in the panel.
    Panel,
    /// A line the person ran in the image's Terminal.
    Terminal,
}

/// What the shell has in hand: one thing at a time, from a take to its result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Running {
    line: Option<Line>,
    /// The Terminal line's tickets while the shell has them.
    tickets: Vec<u32>,
}

impl Running {
    /// The shell took a panel request: a line (`true`), or a Stop for a
    /// background build, which is no line of the person's.
    pub fn took_panel(&mut self, line: bool) {
        self.line = line.then_some(Line::Panel);
        self.tickets.clear();
    }

    /// The shell took `app`'s effects (`tickets`), queued by a person's input
    /// (`user`) or not: only the image's Terminal's, by a person, are a
    /// person's line.
    pub fn took_effects(&mut self, app: &str, user: bool, tickets: Vec<u32>) {
        let terminal = app == STOPS_FROM && user;
        self.line = terminal.then_some(Line::Terminal);
        self.tickets = if terminal { tickets } else { Vec::new() };
    }

    /// Before the shell takes new work: the Terminal line's tickets whose
    /// result never came (the shell moved on without one), for `desktop` to
    /// finish, so the Terminal never waits for ever. Empty otherwise.
    pub fn take_stale(&mut self) -> Vec<u32> {
        if self.line == Some(Line::Terminal) {
            self.line = None;
        }
        core::mem::take(&mut self.tickets)
    }

    /// The shell answered `line`: it no longer runs it.
    pub fn ended(&mut self, line: Line) {
        if self.line == Some(line) {
            self.line = None;
        }
        if line == Line::Terminal {
            self.tickets.clear();
        }
    }

    /// Whether the shell runs `line` now.
    pub fn is(&self, line: Line) -> bool {
        self.line == Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_persons_own_line_counts_and_only_until_its_result() {
        let mut running = Running::default();
        assert!(!running.is(Line::Panel) && !running.is(Line::Terminal));
        running.took_panel(true);
        assert!(running.is(Line::Panel));
        // The Terminal's result does not end the panel's line.
        running.ended(Line::Terminal);
        assert!(running.is(Line::Panel));
        running.ended(Line::Panel);
        assert!(!running.is(Line::Panel));
        running.took_effects(STOPS_FROM, true, alloc::vec![4]);
        assert!(running.is(Line::Terminal));
        running.ended(Line::Terminal);
        assert!(!running.is(Line::Terminal));
        assert!(running.take_stale().is_empty(), "its result came");
    }

    /// #892's review: a Terminal line whose result never came is finished
    /// when the shell takes new work, and no longer counts as running.
    #[test]
    fn a_terminal_line_with_no_result_is_stale_at_the_next_take() {
        let mut running = Running::default();
        running.took_effects(STOPS_FROM, true, alloc::vec![4, 5]);
        assert_eq!(running.take_stale(), [4, 5]);
        assert!(!running.is(Line::Terminal));
        assert!(running.take_stale().is_empty());
    }

    #[test]
    fn another_apps_effects_or_a_build_stop_are_no_persons_line() {
        let mut running = Running::default();
        running.took_effects(STOPS_FROM, false, alloc::vec![1]);
        assert!(!running.is(Line::Terminal), "not by a person");
        assert!(running.take_stale().is_empty());
        running.took_effects("files", true, alloc::vec![2]);
        assert!(!running.is(Line::Terminal), "not the Terminal");
        running.took_panel(false);
        assert!(!running.is(Line::Panel), "a build's Stop");
    }
}
