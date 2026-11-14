//! Which person's line the shell runs now, as `desktop` sees it from the
//! shell's takes and results: a Stop or an Esc interrupts the shell only while
//! that person's own line is the one it runs, never another app's effects or
//! a line still waiting (#892's review).

use crate::effects::{self, Queue, STOPS_FROM, TAKE, TAKE_DURING, Taken};
use alloc::vec::Vec;

/// A person's line the shell may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// A line typed in the panel.
    Panel,
    /// A line the person ran in the image's Terminal.
    Terminal,
}

/// What came of one of the shell's takes of effects ([`Running::take`]).
#[derive(Debug)]
pub struct Took {
    /// The lines taken, if any waited.
    pub taken: Option<Taken>,
    /// The tickets of a Terminal line the shell moved on from with no result,
    /// for `desktop` to finish so the Terminal does not wait for ever.
    pub stale: Vec<u32>,
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

    /// One of the shell's takes of effects from `queue` (`effects take`,
    /// `effects take but ...`, `effects take during`), at most `budget` bytes;
    /// `None` for any other request. A take starts new work: a Terminal line
    /// with no result is stale, and the line taken is the one running. A take
    /// during a line (#877: the apps' reads answered while the model thinks)
    /// is no new unit: the line running, and its tickets, stay.
    pub fn take(&mut self, queue: &mut Queue, text: &str, budget: usize) -> Option<Took> {
        if text == TAKE_DURING {
            return Some(Took {
                taken: queue.take_during(budget),
                stale: Vec::new(),
            });
        }
        let skip = match text == TAKE {
            true => Vec::new(),
            false => effects::parse_take_but(text)?,
        };
        let taken = queue.take_but(budget, &skip);
        let stale = match &taken {
            Some(taken) => {
                let stale = self.take_stale();
                let tickets = taken.lines.iter().filter_map(|l| effects::ticket_of(l));
                self.took_effects(&taken.app, taken.user, tickets.collect());
                stale
            }
            None => Vec::new(),
        };
        Some(Took { taken, stale })
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

    const BUDGET: usize = 1024;

    fn line(ticket: u32) -> alloc::string::String {
        alloc::format!("effect {ticket} notify(\"n\")")
    }

    fn app(took: Took) -> Option<alloc::string::String> {
        took.taken.map(|taken| taken.app)
    }

    /// #877's review: the shell answers the apps' reads while a Terminal line
    /// waits on the model; its take is no new unit.
    #[test]
    fn a_take_during_a_line_keeps_the_line_and_finishes_nothing() {
        let (mut queue, mut running) = (Queue::default(), Running::default());
        queue.put_as(STOPS_FROM, &line(4), true);
        let took = running.take(&mut queue, TAKE, BUDGET).unwrap();
        assert_eq!(app(took).as_deref(), Some(STOPS_FROM));
        assert!(running.is(Line::Terminal));
        queue.put("files", &line(1));
        let took = running.take(&mut queue, TAKE_DURING, BUDGET).unwrap();
        assert!(took.stale.is_empty(), "it finishes nothing");
        assert_eq!(app(took).as_deref(), Some("files"));
        assert!(running.is(Line::Terminal), "the line still runs");
        // Its tickets stay: had its result never come, the next take after
        // it would finish them.
        queue.put("files", &line(2));
        let took = running.take(&mut queue, TAKE, BUDGET).unwrap();
        assert_eq!(took.stale, [4]);
    }

    /// A plain take is new work: a Terminal line with no result is finished.
    #[test]
    fn a_take_after_a_line_with_no_result_finishes_it() {
        let (mut queue, mut running) = (Queue::default(), Running::default());
        queue.put_as(STOPS_FROM, &line(4), true);
        let _ = running.take(&mut queue, TAKE, BUDGET).unwrap();
        queue.put("files", &line(1));
        let took = running
            .take(&mut queue, "effects take but notes", BUDGET)
            .unwrap();
        assert_eq!(took.stale, [4]);
        assert_eq!(app(took).as_deref(), Some("files"));
        assert!(!running.is(Line::Terminal));
        assert!(
            running
                .take(&mut queue, "effects done files\n", BUDGET)
                .is_none()
        );
    }

    /// A person's Terminal line queued while another line runs is not taken
    /// during it, but after, as the line running; the Terminal's other
    /// effects are taken during it.
    #[test]
    fn a_take_during_a_line_leaves_a_persons_terminal_line_for_after() {
        let (mut queue, mut running) = (Queue::default(), Running::default());
        running.took_panel(true);
        queue.put_as(STOPS_FROM, &line(7), true);
        queue.put("files", &line(1));
        let took = running.take(&mut queue, TAKE_DURING, BUDGET).unwrap();
        assert_eq!(app(took).as_deref(), Some("files"));
        let took = running.take(&mut queue, TAKE_DURING, BUDGET).unwrap();
        assert!(took.taken.is_none(), "the Terminal's line waits");
        assert!(running.is(Line::Panel));
        running.ended(Line::Panel);
        let took = running.take(&mut queue, TAKE, BUDGET).unwrap();
        assert_eq!(app(took).as_deref(), Some(STOPS_FROM));
        assert!(running.is(Line::Terminal), "taken after, it runs");
        running.ended(Line::Terminal);
        queue.put(STOPS_FROM, &line(8));
        let took = running.take(&mut queue, TAKE_DURING, BUDGET).unwrap();
        assert_eq!(app(took).as_deref(), Some(STOPS_FROM), "not a person's");
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
