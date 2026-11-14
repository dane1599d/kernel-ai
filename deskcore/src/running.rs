//! Which person's line the shell runs now, as `desktop` sees it from the
//! shell's takes and results: a Stop or an Esc interrupts the shell only while
//! that person's own line is the one it runs, never another app's effects or
//! a line still waiting (#892's review).

use crate::effects::STOPS_FROM;

/// A person's line the shell may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// A line typed in the panel.
    Panel,
    /// A line the person ran in the image's Terminal.
    Terminal,
}

/// What the shell has in hand: one thing at a time, from a take to its result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Running {
    line: Option<Line>,
}

impl Running {
    /// The shell took a panel request: a line (`true`), or a Stop for a
    /// background build, which is no line of the person's.
    pub fn took_panel(&mut self, line: bool) {
        self.line = line.then_some(Line::Panel);
    }

    /// The shell took `app`'s effects, queued by a person's input (`user`) or
    /// not: only the image's Terminal's, by a person, are a person's line.
    pub fn took_effects(&mut self, app: &str, user: bool) {
        self.line = (app == STOPS_FROM && user).then_some(Line::Terminal);
    }

    /// The shell answered `line`: it no longer runs it.
    pub fn ended(&mut self, line: Line) {
        if self.line == Some(line) {
            self.line = None;
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
        running.took_effects(STOPS_FROM, true);
        assert!(running.is(Line::Terminal));
        running.ended(Line::Terminal);
        assert!(!running.is(Line::Terminal));
    }

    #[test]
    fn another_apps_effects_or_a_build_stop_are_no_persons_line() {
        let mut running = Running::default();
        running.took_effects(STOPS_FROM, false);
        assert!(!running.is(Line::Terminal), "not by a person");
        running.took_effects("files", true);
        assert!(!running.is(Line::Terminal), "not the Terminal");
        running.took_panel(false);
        assert!(!running.is(Line::Panel), "a build's Stop");
    }
}
