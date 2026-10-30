//! An app's effects on their way to the agent core and back (P4): calls go one
//! way, so `desktop` holds them between the app's `comphost` and the shell.
//! One request a call, as text:
//!
//! - from an app, `effects put\n<effect lines>` → `ok`: queues its reply's
//!   `effect <ticket> <WAVE>` lines;
//! - from an app, `effects results` → `<ticket>\tok|err\t<text>` lines (or
//!   nothing): what came of its effects, oldest first, taken;
//! - from an app, `effects put user\n<effect lines>` → `ok`: the same, for
//!   lines queued while a person's key or click was handled (comphost says so;
//!   `desktop` takes its word only as the app's own, never for another);
//! - from the shell, `effects take` → `none`, or `app <name>\n<effect lines>`
//!   (`app <name> user\n...` for lines queued by a person's input): one app's
//!   oldest effects, all of one kind;
//! - from the shell, `effects done <name>\n<result lines>` → `ok`.
//!
//! Which app queued the lines is `desktop`'s own record of the caller (the
//! kernel's name for its process), never anything in them. Each app may have
//! [`MAX_QUEUED`] effects waiting; past that a new one is refused at once with
//! an error result, as the agent core refuses one it will not run.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const PUT: &str = "effects put\n";
pub const PUT_USER: &str = "effects put user\n";
pub const RESULTS: &str = "effects results";
pub const TAKE: &str = "effects take";
const DONE: &str = "effects done ";
/// Effects one app may have waiting for the shell.
pub const MAX_QUEUED: usize = 16;
/// Results one app may have waiting for its `comphost`; the oldest go first.
pub const MAX_RESULTS: usize = 32;
/// The longest effect line kept: livecore's limits fit well inside it.
pub const MAX_LINE: usize = 1024;
/// The longest result text sent back.
pub const MAX_TEXT: usize = 200;

/// What one effect came to, as `desktop` carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub ticket: u32,
    pub outcome: Result<String, String>,
}

/// One app's effects as the shell takes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Taken {
    pub app: String,
    /// Whether a person's key or click queued them.
    pub user: bool,
    pub lines: Vec<String>,
}

/// Every app's waiting effects and results.
#[derive(Default)]
pub struct Queue {
    /// Effect lines by app, oldest first, each with whether a person's input
    /// queued it.
    effects: Vec<(String, VecDeque<(bool, String)>)>,
    /// Results by app, oldest first.
    results: Vec<(String, VecDeque<Done>)>,
    /// Where the next take starts looking: round the apps in turn, so one
    /// busy app cannot keep the others waiting.
    next: usize,
}

impl Queue {
    /// Queues `app`'s effect lines, not from a person's input.
    pub fn put(&mut self, app: &str, lines: &str) -> usize {
        self.put_as(app, lines, false)
    }

    /// Queues `app`'s effect lines, queued by a person's input if `user`; how
    /// many were refused as over the limit or out of shape, each of which gets
    /// an error result.
    pub fn put_as(&mut self, app: &str, lines: &str, user: bool) -> usize {
        let mut refused = Vec::new();
        let waiting = entry(&mut self.effects, app);
        for line in lines.lines() {
            let Some(ticket) = ticket_of(line) else {
                continue;
            };
            let why = if line.len() > MAX_LINE {
                "the effect is too long"
            } else if waiting.len() == MAX_QUEUED {
                "too many effects are waiting; try again"
            } else {
                waiting.push_back((user, line.to_string()));
                continue;
            };
            refused.push(Done {
                ticket,
                outcome: Err(String::from(why)),
            });
        }
        let count = refused.len();
        for done in refused {
            self.finish(app, done);
        }
        count
    }

    /// One app's oldest effects, all queued the same way, at most `budget` bytes
    /// of lines (always at least one), the apps taking turns; `None` when
    /// nothing waits.
    pub fn take(&mut self, budget: usize) -> Option<Taken> {
        let count = self.effects.len();
        let at = (0..count)
            .map(|step| (self.next + step) % count)
            .find(|&at| !self.effects[at].1.is_empty())?;
        self.next = (at + 1) % count;
        let (app, waiting) = &mut self.effects[at];
        let user = waiting.front().is_some_and(|(user, _)| *user);
        let mut lines = Vec::new();
        let mut used = 0;
        while let Some((by, line)) = waiting.front() {
            if *by != user || (!lines.is_empty() && used + line.len() + 1 > budget) {
                break;
            }
            used += line.len() + 1;
            lines.extend(waiting.pop_front().map(|(_, line)| line));
        }
        Some(Taken {
            app: app.clone(),
            user,
            lines,
        })
    }

    /// Records what came of one of `app`'s effects.
    pub fn finish(&mut self, app: &str, done: Done) {
        let results = entry(&mut self.results, app);
        if results.len() == MAX_RESULTS {
            results.pop_front();
        }
        results.push_back(done);
    }

    /// `app`'s results, oldest first; taking them empties its list.
    pub fn results(&mut self, app: &str) -> Vec<Done> {
        self.results
            .iter_mut()
            .find(|(name, _)| name == app)
            .map(|(_, results)| results.drain(..).collect())
            .unwrap_or_default()
    }
}

fn entry<'a, T>(all: &'a mut Vec<(String, VecDeque<T>)>, app: &str) -> &'a mut VecDeque<T> {
    let at = match all.iter().position(|(name, _)| name == app) {
        Some(at) => at,
        None => {
            all.push((app.to_string(), VecDeque::new()));
            all.len() - 1
        }
    };
    &mut all[at].1
}

/// The app's side: the tickets it sent and has no result for yet. A result
/// reaches the app only for a ticket in here, once; anything else `desktop`
/// answers is dropped.
#[derive(Default)]
pub struct Sent {
    tickets: VecDeque<u32>,
}

impl Sent {
    /// Records the tickets of the effect lines `desktop` took; the oldest go
    /// past [`MAX_RESULTS`], as `desktop` keeps no more results than that.
    pub fn record(&mut self, lines: &str) {
        for ticket in lines.lines().filter_map(ticket_of) {
            if self.tickets.len() == MAX_RESULTS {
                self.tickets.pop_front();
            }
            self.tickets.push_back(ticket);
        }
    }

    /// Whether a result is still awaited.
    pub fn awaits(&self) -> bool {
        !self.tickets.is_empty()
    }

    /// The results for tickets sent, each taken off the record, and the
    /// tickets of the rest.
    pub fn accept(&mut self, results: Vec<Done>) -> (Vec<Done>, Vec<u32>) {
        let mut accepted = Vec::new();
        let mut stray = Vec::new();
        for done in results {
            match self.tickets.iter().position(|t| *t == done.ticket) {
                Some(at) => {
                    self.tickets.remove(at);
                    accepted.push(done);
                }
                None => stray.push(done.ticket),
            }
        }
        (accepted, stray)
    }
}

/// The ticket of an `effect <ticket> <WAVE>` line.
pub fn ticket_of(line: &str) -> Option<u32> {
    let (ticket, _) = line.strip_prefix("effect ")?.split_once(' ')?;
    ticket.parse().ok()
}

/// The answer to [`TAKE`].
pub fn encode_take(taken: Option<&Taken>) -> String {
    match taken {
        Some(Taken { app, user, lines }) => {
            let by = if *user { " user" } else { "" };
            format!("app {app}{by}\n{}", lines.join("\n"))
        }
        None => String::from("none"),
    }
}

/// [`encode_take`] read back: the app, whether a person's input queued its
/// lines, and the lines; `Some(None)` for none, `None` for anything else.
pub fn parse_take(text: &str) -> Option<Option<(&str, bool, &str)>> {
    if text == "none" {
        return Some(None);
    }
    let (head, lines) = text.split_once('\n').unwrap_or((text, ""));
    let head = head.strip_prefix("app ")?;
    Some(Some(match head.strip_suffix(" user") {
        Some(app) => (app, true, lines),
        None => (head, false, lines),
    }))
}

/// Results as lines: `<ticket>\tok|err\t<text>`.
pub fn encode_results(results: &[Done]) -> String {
    let lines: Vec<String> = results
        .iter()
        .map(|done| {
            let (word, text) = match &done.outcome {
                Ok(text) => ("ok", text),
                Err(text) => ("err", text),
            };
            format!("{}\t{word}\t{}", done.ticket, one_line(text))
        })
        .collect();
    lines.join("\n")
}

/// [`encode_results`] read back; lines out of shape are skipped.
pub fn parse_results(text: &str) -> Vec<Done> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let ticket = parts.next()?.parse().ok()?;
            let word = parts.next()?;
            let text = parts.next().unwrap_or("").to_string();
            let outcome = match word {
                "ok" => Ok(text),
                "err" => Err(text),
                _ => return None,
            };
            Some(Done { ticket, outcome })
        })
        .collect()
}

/// The shell's report on `app`'s effects, as its request.
pub fn encode_done(app: &str, results: &[Done]) -> String {
    format!("{DONE}{app}\n{}", encode_results(results))
}

/// An [`encode_done`] request read back; `None` for anything else.
pub fn parse_done(text: &str) -> Option<(&str, Vec<Done>)> {
    let body = text.strip_prefix(DONE)?;
    let (app, lines) = body.split_once('\n').unwrap_or((body, ""));
    Some((app, parse_results(lines)))
}

fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").replace('\t', " ");
    let mut end = line.len().min(MAX_TEXT);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ticket: u32) -> String {
        format!("effect {ticket} notify(\"n\")")
    }

    fn taken(app: &str, user: bool, lines: &[String]) -> Taken {
        Taken {
            app: String::from(app),
            user,
            lines: lines.to_vec(),
        }
    }

    #[test]
    fn a_persons_lines_are_taken_apart_from_the_rest_and_keep_their_flag() {
        let mut queue = Queue::default();
        queue.put_as("terminal-window", &line(1), true);
        queue.put("terminal-window", &line(2));
        queue.put_as(
            "terminal-window",
            &format!("{}\n{}", line(3), line(4)),
            true,
        );
        let first = queue.take(4096).unwrap();
        assert_eq!(first, taken("terminal-window", true, &[line(1)]));
        assert_eq!(
            queue.take(4096),
            Some(taken("terminal-window", false, &[line(2)]))
        );
        assert_eq!(
            queue.take(4096),
            Some(taken("terminal-window", true, &[line(3), line(4)]))
        );
        assert_eq!(queue.take(4096), None);
    }

    #[test]
    fn a_refused_persons_line_still_gets_its_error_result() {
        let mut queue = Queue::default();
        let long = format!("effect 7 notify(\"{}\")", "x".repeat(MAX_LINE));
        assert_eq!(queue.put_as("terminal-window", &long, true), 1);
        assert_eq!(queue.results("terminal-window")[0].ticket, 7);
        assert_eq!(queue.take(4096), None);
    }

    #[test]
    fn the_flag_rides_the_wire_only_as_the_word_user() {
        let user = taken("terminal-window", true, &[line(1)]);
        let text = encode_take(Some(&user));
        assert!(text.starts_with("app terminal-window user\n"));
        assert_eq!(
            parse_take(&text),
            Some(Some(("terminal-window", true, line(1).as_str())))
        );
        let plain = taken("terminal-window", false, &[line(1)]);
        assert_eq!(
            parse_take(&encode_take(Some(&plain))),
            Some(Some(("terminal-window", false, line(1).as_str())))
        );
        // The word is a suffix of the app, not part of a line's text.
        let sneaky = taken(
            "files",
            false,
            &[String::from("effect 1 notify(\"x user\")")],
        );
        assert_eq!(
            parse_take(&encode_take(Some(&sneaky))).map(|t| t.map(|(a, u, _)| (a, u))),
            Some(Some(("files", false)))
        );
    }

    #[test]
    fn effects_wait_by_app_and_the_shell_takes_one_apps_at_a_time() {
        let mut queue = Queue::default();
        assert_eq!(queue.put("files", &format!("{}\n{}", line(1), line(2))), 0);
        assert_eq!(queue.put("monitor", &line(9)), 0);
        assert_eq!(
            queue.take(4096),
            Some(taken("files", false, &[line(1), line(2)]))
        );
        assert_eq!(queue.take(4096), Some(taken("monitor", false, &[line(9)])));
        assert_eq!(queue.take(4096), None);
    }

    #[test]
    fn apps_take_turns() {
        let mut queue = Queue::default();
        let lines: Vec<String> = (0..4).map(line).collect();
        queue.put(
            "files",
            &lines.join(
                "
",
            ),
        );
        queue.put("monitor", &line(9));
        let budget = line(0).len() + 1;
        let order: Vec<String> =
            core::iter::from_fn(|| queue.take(budget).map(|t| t.app)).collect();
        assert_eq!(order, ["files", "monitor", "files", "files", "files"]);
    }

    #[test]
    fn a_take_keeps_to_its_budget_but_always_moves() {
        let mut queue = Queue::default();
        queue.put("files", &format!("{}\n{}", line(1), line(2)));
        assert_eq!(queue.take(1).map(|t| t.lines), Some(alloc::vec![line(1)]));
        assert_eq!(queue.take(1).map(|t| t.lines), Some(alloc::vec![line(2)]));
    }

    #[test]
    fn past_the_limit_an_effect_is_refused_with_a_result() {
        let mut queue = Queue::default();
        let lines: Vec<String> = (0..=MAX_QUEUED as u32).map(line).collect();
        assert_eq!(queue.put("files", &lines.join("\n")), 1);
        let results = queue.results("files");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].ticket, MAX_QUEUED as u32);
        assert!(results[0].outcome.is_err());
        assert_eq!(
            queue.take(usize::MAX).map(|t| t.lines.len()),
            Some(MAX_QUEUED)
        );
    }

    #[test]
    fn lines_that_are_not_effects_are_ignored() {
        let mut queue = Queue::default();
        assert_eq!(queue.put("files", "result ok\neffect x y\n"), 0);
        assert_eq!(queue.take(4096), None);
    }

    #[test]
    fn results_go_back_to_their_app_only_and_are_bounded() {
        let mut queue = Queue::default();
        for ticket in 0..MAX_RESULTS as u32 + 2 {
            queue.finish(
                "files",
                Done {
                    ticket,
                    outcome: Ok(String::new()),
                },
            );
        }
        assert!(queue.results("monitor").is_empty());
        let results = queue.results("files");
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[0].ticket, 2);
        assert!(queue.results("files").is_empty());
    }

    #[test]
    fn an_app_takes_results_only_for_tickets_it_sent_and_once() {
        let mut sent = Sent::default();
        assert!(!sent.awaits());
        sent.record(&format!("{}\n{}\nresult ok", line(4), line(5)));
        assert!(sent.awaits());
        let done = |ticket| Done {
            ticket,
            outcome: Ok(String::new()),
        };
        let (accepted, stray) = sent.accept(alloc::vec![done(5), done(9), done(5)]);
        assert_eq!(accepted, [done(5)]);
        assert_eq!(stray, [9, 5]);
        assert!(sent.awaits());
        let (accepted, _) = sent.accept(alloc::vec![done(4)]);
        assert_eq!(accepted, [done(4)]);
        assert!(!sent.awaits());
    }

    #[test]
    fn the_record_is_bounded() {
        let mut sent = Sent::default();
        let lines: Vec<String> = (0..=MAX_RESULTS as u32).map(line).collect();
        sent.record(&lines.join("\n"));
        let done = |ticket| Done {
            ticket,
            outcome: Ok(String::new()),
        };
        let (accepted, stray) = sent.accept(alloc::vec![done(0), done(1)]);
        assert_eq!((accepted.len(), stray), (1, alloc::vec![0]));
    }

    #[test]
    fn the_wire_reads_back() {
        let taken = taken("files", false, &[line(1), line(2)]);
        let text = encode_take(Some(&taken));
        let lines = format!("{}\n{}", line(1), line(2));
        assert_eq!(
            parse_take(&text),
            Some(Some(("files", false, lines.as_str())))
        );
        assert_eq!(parse_take(&encode_take(None)), Some(None));
        assert_eq!(parse_take("app"), None);

        let results = alloc::vec![
            Done {
                ticket: 1,
                outcome: Ok(String::from("Done: x")),
            },
            Done {
                ticket: 2,
                outcome: Err(String::from("no\tsuch\nrow")),
            },
        ];
        let request = encode_done("files", &results);
        let (app, back) = parse_done(&request).expect("a done request");
        assert_eq!(app, "files");
        assert_eq!(back[0], results[0]);
        assert_eq!(back[1].outcome, Err(String::from("no such")));
        assert_eq!(parse_done("effects take"), None);
    }
}
