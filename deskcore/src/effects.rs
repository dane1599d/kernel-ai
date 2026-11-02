//! An app's effects on their way to the agent core and back (P4): calls go one
//! way, so `desktop` holds them between the app's `comphost` and the shell.
//! One request a call, as text:
//!
//! - from an app, `effects put\n<effect lines>` → `ok`: queues its reply's
//!   `effect <ticket> <WAVE>` lines;
//! - from an app, `effects results` → `<ticket>\tok|err\t<text>` lines (or
//!   nothing): what came of its effects, oldest first, taken. A text of several
//!   lines (a folder's listing) is `okm` or `errm` and has `\`, tab, return and
//!   newline escaped, cut at the end of a line within the limit below;
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
/// The transport's ceiling for one result's text, which only a person's input
/// (a Terminal line's answer) may use: the shell cuts the results of a batch a
/// person's key or click queued here ([`encode_done_as`]) and every other
/// result at [`MAX_TEXT`].
pub const MAX_LINE_TEXT: usize = 2048;
/// What one message to or from `desktop` may carry of results, less room for
/// the framing (`kabi::MAX_MESSAGE`).
pub const MESSAGE_BUDGET: usize = kabi::MAX_MESSAGE - 64;

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

    /// `app`'s oldest results that fit `budget` bytes once encoded (always at
    /// least one), taken off its list; the rest wait for the next call.
    pub fn results(&mut self, app: &str, budget: usize) -> Vec<Done> {
        let Some((_, waiting)) = self.results.iter_mut().find(|(name, _)| name == app) else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        let mut used = 0;
        while let Some(done) = waiting.front() {
            let size = encoded_len(done);
            if !taken.is_empty() && used + size > budget {
                break;
            }
            used += size;
            taken.extend(waiting.pop_front());
        }
        taken
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

/// Results as lines: `<ticket>\tok|err|okm|errm\t<text>`, each text one line cut
/// at [`MAX_LINE_TEXT`] (a text of several lines escaped, as the module says):
/// what `desktop` hands an app, the shell having cut each already.
pub fn encode_results(results: &[Done]) -> String {
    encode_results_at(results, MAX_LINE_TEXT)
}

fn encode_results_at(results: &[Done], cap: usize) -> String {
    let lines: Vec<String> = results
        .iter()
        .map(|done| {
            let (word, text) = wire(done, cap);
            format!("{}\t{word}\t{text}", done.ticket)
        })
        .collect();
    lines.join("\n")
}

/// A result's word and its text as one wire line. A text of one line goes as it
/// is (`ok`, `err`), cut at `cap`. A text of several (a folder's listing) goes
/// as `okm` or `errm`, with `\`, tab, return and newline escaped (`\\`, `\t`,
/// `\r`, `\n`) and cut at the end of a line within `cap`, so a name is never
/// left half; a word of the one kind or the other tells the reader which.
fn wire(done: &Done, cap: usize) -> (&'static str, String) {
    let (words, text) = match &done.outcome {
        Ok(text) => (("ok", "okm"), text),
        Err(text) => (("err", "errm"), text),
    };
    match text.lines().nth(1) {
        None => (words.0, one_line(text, cap)),
        Some(_) => (words.1, escaped_lines(text, cap)),
    }
}

/// Bytes one result takes in [`encode_results`], its newline included.
fn encoded_len(done: &Done) -> usize {
    let (word, text) = wire(done, MAX_LINE_TEXT);
    done.ticket.to_string().len() + word.len() + 3 + text.len()
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
                "okm" => Ok(unescape(&text)),
                "errm" => Err(unescape(&text)),
                _ => return None,
            };
            Some(Done { ticket, outcome })
        })
        .collect()
}

/// `text` with `\`, tab, return and newline escaped, cut at the end of the last
/// whole line that fits `cap` bytes; a first line longer than `cap` is cut inside
/// (never inside an escape).
fn escaped_lines(text: &str, cap: usize) -> String {
    let mut out = String::new();
    let mut whole = 0;
    for c in text.chars() {
        let escape = match c {
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            _ => None,
        };
        let size = escape.map_or(c.len_utf8(), str::len);
        if out.len() + size > cap {
            if whole > 0 {
                out.truncate(whole);
            }
            return out;
        }
        match escape {
            Some(escape) => out.push_str(escape),
            None => out.push(c),
        }
        if c == '\n' {
            whole = out.len();
        }
    }
    out
}

/// [`escaped_lines`] read back. A backslash before anything else, or at the end,
/// stays as it is.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let unescaped = match (c, chars.peek()) {
            ('\\', Some('\\')) => '\\',
            ('\\', Some('n')) => '\n',
            ('\\', Some('r')) => '\r',
            ('\\', Some('t')) => '\t',
            _ => {
                out.push(c);
                continue;
            }
        };
        chars.next();
        out.push(unescaped);
    }
    out
}

/// The shell's report on `app`'s effects, as its request: each text cut at
/// [`MAX_TEXT`].
pub fn encode_done(app: &str, results: &[Done]) -> String {
    encode_done_as(app, false, results)
}

/// [`encode_done`] for a batch that a person's input queued if `user`: its
/// texts may be [`MAX_LINE_TEXT`] long. The request is one message, so the
/// results share what is left of it.
pub fn encode_done_as(app: &str, user: bool, results: &[Done]) -> String {
    let cap = if user { MAX_LINE_TEXT } else { MAX_TEXT };
    let room = MESSAGE_BUDGET.saturating_sub(DONE.len() + app.len() + 1);
    // A ticket, two tabs, the word and the newline take well under 24 bytes.
    let each = (room / results.len().max(1)).saturating_sub(24).max(1);
    format!("{DONE}{app}\n{}", encode_results_at(results, cap.min(each)))
}

/// An [`encode_done`] request read back; `None` for anything else.
pub fn parse_done(text: &str) -> Option<(&str, Vec<Done>)> {
    let body = text.strip_prefix(DONE)?;
    let (app, lines) = body.split_once('\n').unwrap_or((body, ""));
    Some((app, parse_results(lines)))
}

fn one_line(text: &str, cap: usize) -> String {
    let line = text.lines().next().unwrap_or("").replace('\t', " ");
    let mut end = line.len().min(cap);
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

    fn done_text(ticket: u32, text: &str) -> Done {
        Done {
            ticket,
            outcome: Ok(text.to_string()),
        }
    }

    /// Shell to desktop to app, as the three of them pass a result.
    fn through(app: &str, user: bool, results: &[Done]) -> Vec<Done> {
        let request = encode_done_as(app, user, results);
        assert!(
            request.len() <= kabi::MAX_MESSAGE,
            "{} bytes",
            request.len()
        );
        let (name, parsed) = parse_done(&request).unwrap();
        let mut queue = Queue::default();
        for done in parsed {
            queue.finish(name, done);
        }
        let mut back = Vec::new();
        loop {
            let batch = queue.results(name, MESSAGE_BUDGET);
            if batch.is_empty() {
                return back;
            }
            let text = encode_results(&batch);
            assert!(text.len() <= MESSAGE_BUDGET + 64, "{} bytes", text.len());
            back.extend(parse_results(&text));
        }
    }

    #[test]
    fn a_two_kib_result_from_a_persons_batch_arrives_whole() {
        let text = "x".repeat(MAX_LINE_TEXT);
        let back = through("terminal-window", true, &[done_text(1, &text)]);
        assert_eq!(back, [done_text(1, &text)]);
        // Over the ceiling it is cut there, at a character.
        let over = "\u{e9}".repeat(MAX_LINE_TEXT);
        let back = through("terminal-window", true, &[done_text(2, &over)]);
        let Ok(cut) = &back[0].outcome else { panic!() };
        assert!(cut.len() <= MAX_LINE_TEXT && over.starts_with(cut.as_str()));
    }

    #[test]
    fn any_other_result_is_still_cut_at_two_hundred() {
        let text = "y".repeat(MAX_LINE_TEXT);
        let back = through("files", false, &[done_text(1, &text)]);
        assert_eq!(back[0].outcome, Ok("y".repeat(MAX_TEXT)));
    }

    #[test]
    fn results_over_one_message_come_back_across_calls_and_none_is_lost() {
        let mut queue = Queue::default();
        let text = "z".repeat(MAX_LINE_TEXT);
        for ticket in 1..=6 {
            queue.finish("terminal-window", done_text(ticket, &text));
        }
        let first = queue.results("terminal-window", MESSAGE_BUDGET);
        assert!(!first.is_empty() && first.len() < 6, "{}", first.len());
        assert!(encode_results(&first).len() <= MESSAGE_BUDGET);
        let rest = queue.results("terminal-window", MESSAGE_BUDGET);
        let tickets: Vec<u32> = first.iter().chain(&rest).map(|d| d.ticket).collect();
        assert_eq!(tickets, [1, 2, 3, 4, 5, 6][..tickets.len()]);
        // However small the budget, one result still moves.
        queue.finish("terminal-window", done_text(7, "a"));
        queue.finish("terminal-window", done_text(8, "b"));
        assert_eq!(queue.results("terminal-window", 0).len(), 1);
        assert_eq!(queue.results("terminal-window", 0).len(), 1);
        assert!(queue.results("terminal-window", 0).is_empty());
    }

    #[test]
    fn a_batch_of_many_long_results_shares_one_request() {
        let text = "w".repeat(MAX_LINE_TEXT);
        let batch: Vec<Done> = (1..=16).map(|t| done_text(t, &text)).collect();
        let back = through("terminal-window", true, &batch);
        assert_eq!(back.len(), 16);
        assert!(
            back.iter()
                .all(|d| matches!(&d.outcome, Ok(t) if !t.is_empty()))
        );
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
        assert_eq!(queue.results("terminal-window", 4096)[0].ticket, 7);
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
        let results = queue.results("files", MESSAGE_BUDGET);
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
        assert!(queue.results("monitor", MESSAGE_BUDGET).is_empty());
        let results = queue.results("files", MESSAGE_BUDGET);
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[0].ticket, 2);
        assert!(queue.results("files", MESSAGE_BUDGET).is_empty());
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
        // A text of several lines goes whole, on one line.
        assert_eq!(back[1], results[1]);
        assert_eq!(parse_done("effects take"), None);
    }

    fn ok(text: &str) -> Done {
        Done {
            ticket: 7,
            outcome: Ok(String::from(text)),
        }
    }

    fn through_the_wire(done: &Done, cap: usize) -> Done {
        let wire = encode_results_at(core::slice::from_ref(done), cap);
        assert_eq!(wire.lines().count(), 1, "{wire:?}");
        parse_results(&wire).remove(0)
    }

    /// The desktop tour's Files window (`03-files`): `notes.txt`, `plan.txt` and
    /// `ideas.txt` were created, `fs_list` answered all three, and the app was
    /// told only the first line. Through the shell, `desktop` and `comphost`, an
    /// app's own listing and a person's both reach it whole.
    #[test]
    fn a_folder_of_three_files_reaches_the_app_with_all_three() {
        let listing = "ideas.txt\nnotes.txt\nplan.txt\n";
        for user in [false, true] {
            let got = through("files-window", user, &[ok(listing)]);
            assert_eq!(got, alloc::vec![ok(listing)], "user {user}");
        }
        // With a folder in it, and among other results of the same reply.
        let mixed = alloc::vec![ok("Done: x"), ok("docs/\nnotes.txt\n"), ok("plain")];
        assert_eq!(through("files-window", false, &mixed), mixed);
    }

    #[test]
    fn a_listing_arrives_with_every_name() {
        let listing = "ideas.txt\nnotes.txt\nplan.txt\n";
        let wire = encode_results(&[ok(listing)]);
        assert_eq!(wire, "7\tokm\tideas.txt\\nnotes.txt\\nplan.txt\\n");
        assert_eq!(parse_results(&wire), alloc::vec![ok(listing)]);
        // One line, with or without its newline, is as it was before.
        assert_eq!(encode_results(&[ok("Done: x")]), "7\tok\tDone: x");
        assert_eq!(encode_results(&[ok("Done: x\n")]), "7\tok\tDone: x");
        assert_eq!(encode_results(&[ok("")]), "7\tok\t");
    }

    #[test]
    fn several_lines_keep_their_backslashes_tabs_returns_and_letters() {
        for text in [
            "a\\nb\nc",
            "back\\slash\nand\ttab\r\nand\rreturn",
            "é\u{2028}\nfolder/\nname with spaces ",
            "\\\n\\\\\n\\n\n\\",
            "ends in a backslash\\\nsecond",
        ] {
            let done = ok(text);
            assert_eq!(through_the_wire(&done, MAX_LINE_TEXT), done, "{text:?}");
        }
        let err = Done {
            ticket: 2,
            outcome: Err(String::from("no\nsuch\trow")),
        };
        assert_eq!(through_the_wire(&err, MAX_TEXT), err);
    }

    #[test]
    fn a_long_listing_is_cut_between_names_never_inside_one() {
        let names: Vec<String> = (0..60).map(|i| format!("file-number-{i:03}.txt")).collect();
        let listing = format!("{}\n", names.join("\n"));
        let got = through_the_wire(&ok(&listing), MAX_TEXT);
        let Ok(text) = got.outcome else {
            panic!("an ok result");
        };
        assert!(text.len() < MAX_TEXT, "{} bytes", text.len());
        assert!(text.ends_with('\n'));
        let kept: Vec<&str> = text.lines().collect();
        assert!(kept.len() > 5 && kept.len() < names.len());
        assert!(kept.iter().zip(&names).all(|(kept, name)| kept == name));
        // A first line longer than the cap is cut inside, at a character.
        let long = format!("{}\nsecond", "é".repeat(MAX_TEXT));
        let Ok(text) = through_the_wire(&ok(&long), MAX_TEXT).outcome else {
            panic!("an ok result");
        };
        assert!(text.len() <= MAX_TEXT && text.chars().all(|c| c == 'é'));
        // Nor is an escape ever cut in two.
        let slashes = format!("{}\nx", "\\".repeat(MAX_TEXT));
        let Ok(text) = through_the_wire(&ok(&slashes), MAX_TEXT).outcome else {
            panic!("an ok result");
        };
        assert!(text.chars().all(|c| c == '\\') && text.len() == MAX_TEXT / 2);
    }

    #[test]
    fn the_budget_counts_a_listing_as_it_goes_on_the_wire() {
        let mut queue = Queue::default();
        let big = ok(&"n
"
        .repeat(2000));
        // The budget holds the listing as cut and sent, and not a byte more.
        let size = encode_results(core::slice::from_ref(&big)).len() + 1;
        queue.finish("files", big.clone());
        queue.finish("files", ok("Done: x"));
        let taken = queue.results("files", size);
        assert_eq!(
            taken,
            alloc::vec![big],
            "the second waits for the next call"
        );
        assert_eq!(queue.results("files", size), alloc::vec![ok("Done: x")]);
    }

    #[test]
    fn an_escape_the_reader_does_not_know_stays_as_it_is() {
        for (wire, text) in [
            ("7\tokm\ta\\qb\\", "a\\qb\\"),
            ("7\tokm\t\\\\n", "\\n"),
            ("7\tokm\t", ""),
            ("7\tokm\t\\\\", "\\"),
        ] {
            assert_eq!(parse_results(wire), alloc::vec![ok(text)], "{wire:?}");
        }
        // A word the reader does not know is skipped, not guessed at.
        assert_eq!(parse_results("7\tokx\ta"), Vec::new());
        assert_eq!(parse_results("7\tOKM\ta"), Vec::new());
        // Only a text of several lines is written with the m.
        assert!(!encode_results(&[ok("a b")]).contains("okm"));
    }

    #[test]
    fn any_text_goes_through_and_comes_out_the_same_or_cut_to_whole_lines() {
        // A fixed xorshift, so a failure is the same failure next time.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet = ['a', 'b', ' ', '\n', '\n', '\\', '\t', '\r', 'é', 'n', 't'];
        for _ in 0..2000 {
            let len = (next() % 400) as usize;
            let text: String = (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            let cap = 1 + (next() % 300) as usize;
            let got = through_the_wire(&ok(&text), cap);
            let Ok(got) = got.outcome else {
                panic!("an ok result");
            };
            if text.lines().nth(1).is_none() {
                // One line: as `one_line` always did.
                assert_eq!(
                    got.replace(' ', "\t"),
                    one_line(&text, cap).replace(' ', "\t")
                );
                continue;
            }
            assert!(text.starts_with(&got), "{text:?} {got:?}");
            // What is kept ends where a line ends, or is all there is.
            assert!(got == text || got.ends_with('\n') || !got.contains('\n'));
        }
    }
}
