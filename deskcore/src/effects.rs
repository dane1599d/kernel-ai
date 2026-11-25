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
//! - from the shell, `effects done <name>\n<result lines>` → `ok`. A line
//!   the shell gave back and still waiting goes when its ticket is answered:
//!   the shell told the app it did not run (#938), so it never runs after.
//!   Only a line given back: tickets start again when an app reloads, so a
//!   late result for an old ticket never withdraws a new line of that number;
//! - from the shell, `effects take but <name> <name>...` → as `effects take`,
//!   skipping those apps: ones whose requests it holds back for now;
//! - from the shell, `effects take during` → as `effects take`, while it runs
//!   a line and answers the apps' reads meanwhile (#877). It starts no new
//!   unit: the line `desktop` sees running, and its tickets, stay
//!   (`running::Running::take`). A person's line in [`STOPS_FROM`] is not
//!   handed out: it waits for a take after the line, which starts it;
//! - from the shell, `effects take during but <name> <name>...` → as
//!   `effects take during`, skipping those apps: ones whose requests it put
//!   back until the line ends;
//! - from the shell, `effects take app <name>` → as `effects take`, from that
//!   app's queue only: after the line, the shell takes again what it put
//!   back, oldest first;
//! - from the shell, `effects back app <name>[ user]\n<effect lines>` → `ok`:
//!   lines it took and will not run yet, in `effects take`'s form, back at the
//!   front of that app's queue in their order (#890). Only a line taken and not
//!   answered or put back since goes back, with the flag it was taken with, so
//!   a back sent twice queues nothing twice. A request the shell holds
//!   while the model thinks then stays in `desktop`, so a shell that dies
//!   mid-line loses none: the restarted shell takes it.
//!
//! Which app queued the lines is `desktop`'s own record of the caller (the
//! kernel's name for its process), never anything in them. Each app may have
//! [`MAX_QUEUED`] effects waiting; past that a new one is refused at once with
//! an error result, as the agent core refuses one it will not run.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::live::NOTICE_TICKET;

pub const PUT: &str = "effects put\n";
pub const PUT_USER: &str = "effects put user\n";
pub const RESULTS: &str = "effects results";
pub const TAKE: &str = "effects take";
pub const TAKE_BUT: &str = "effects take but ";
pub const TAKE_DURING: &str = "effects take during";
pub const TAKE_DURING_BUT: &str = "effects take during but ";
pub const TAKE_APP: &str = "effects take app ";
pub const BACK: &str = "effects back ";
const DONE: &str = "effects done ";
/// Effects one app may have waiting for the shell.
pub const MAX_QUEUED: usize = 16;
/// Lines taken and not answered that may be put back; past it the oldest can
/// no longer be (a shell that took them and died answers none).
pub const MAX_TAKEN: usize = 64;
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
    /// Lines the shell took and has not answered or put back: app, ticket and
    /// whether a person's input queued it, oldest first.
    taken: VecDeque<(String, u32, bool)>,
    /// Lines the shell put back that wait again, by app and ticket, oldest
    /// first, at most [`MAX_TAKEN`]: the only ones a result withdraws.
    given_back: VecDeque<(String, u32)>,
}

impl Queue {
    /// Takes back `app`'s lines a person queued that the shell has not taken
    /// yet (a Stop before its line ran): each one's ticket, oldest first. Lines
    /// not from a person stay.
    pub fn withdraw_users(&mut self, app: &str) -> Vec<u32> {
        let Some((_, waiting)) = self.effects.iter_mut().find(|(a, _)| a == app) else {
            return Vec::new();
        };
        let mut tickets = Vec::new();
        waiting.retain(|(user, line)| match (*user, ticket_of(line)) {
            (true, Some(ticket)) => {
                tickets.push(ticket);
                false
            }
            _ => true,
        });
        tickets
    }

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
            } else if waiting.len() >= MAX_QUEUED {
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
        self.take_but(budget, &[])
    }

    /// [`Queue::take`] while the shell runs a line: a person's line queued in
    /// [`STOPS_FROM`] (the front of its queue, a person's) is not handed out,
    /// and waits for a take after the line.
    pub fn take_during(&mut self, budget: usize) -> Option<Taken> {
        self.take_during_but(budget, &[])
    }

    /// [`Queue::take_during`], skipping the apps in `skip` too: ones whose
    /// requests the shell put back until the line ends.
    pub fn take_during_but(&mut self, budget: usize, skip: &[&str]) -> Option<Taken> {
        let line_waits = self.effects.iter().any(|(app, waiting)| {
            app == STOPS_FROM && waiting.front().is_some_and(|(user, _)| *user)
        });
        let mut skip = skip.to_vec();
        if line_waits {
            skip.push(STOPS_FROM);
        }
        self.take_but(budget, &skip)
    }

    /// [`Queue::take`] from `app`'s queue only; `None` when it has nothing
    /// waiting.
    pub fn take_app(&mut self, budget: usize, app: &str) -> Option<Taken> {
        let at = self
            .effects
            .iter()
            .position(|(name, waiting)| name == app && !waiting.is_empty())?;
        Some(self.take_at(at, budget))
    }

    /// [`Queue::take`], skipping the apps in `skip`.
    pub fn take_but(&mut self, budget: usize, skip: &[&str]) -> Option<Taken> {
        let count = self.effects.len();
        let at = (0..count)
            .map(|step| (self.next + step) % count)
            .find(|&at| {
                let (app, waiting) = &self.effects[at];
                !waiting.is_empty() && !skip.contains(&app.as_str())
            })?;
        self.next = (at + 1) % count;
        Some(self.take_at(at, budget))
    }

    /// The oldest lines of the app at `at`, which has some, all queued the
    /// same way, at most `budget` bytes (always at least one).
    fn take_at(&mut self, at: usize, budget: usize) -> Taken {
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
        let app = app.clone();
        for ticket in lines.iter().filter_map(|line| ticket_of(line)) {
            // Taken, so no longer waiting as given back.
            self.given_back
                .retain(|(a, t)| !(*a == app && *t == ticket));
            // Tickets start again when an app or its comphost reloads: an
            // older entry for the same ticket is gone.
            self.taken.retain(|(a, t, _)| !(*a == app && *t == ticket));
            if self.taken.len() == MAX_TAKEN {
                self.taken.pop_front();
            }
            self.taken.push_back((app.clone(), ticket, user));
        }
        Taken { app, user, lines }
    }

    /// Puts lines the shell took back at the front of their app's queue, in
    /// their order and with the flag they were taken with, ahead of anything
    /// queued since: how many were kept. Only a line taken and not answered
    /// or put back since is kept, so a back sent twice queues nothing twice;
    /// none is refused for the queue's limit, as each was queued once already
    /// (the limit still holds for new lines: [`Queue::put_as`]).
    pub fn put_back(&mut self, taken: Taken) -> usize {
        let mut back = Vec::new();
        for line in taken.lines {
            let Some(ticket) = ticket_of(&line) else {
                continue;
            };
            let at = self
                .taken
                .iter()
                .rposition(|(app, t, _)| *app == taken.app && *t == ticket);
            if let Some((_, _, user)) = at.and_then(|at| self.taken.remove(at)) {
                back.push((user, line));
                self.given_back
                    .retain(|(a, t)| !(*a == taken.app && *t == ticket));
                if self.given_back.len() == MAX_TAKEN {
                    self.given_back.pop_front();
                }
                self.given_back.push_back((taken.app.clone(), ticket));
            }
        }
        let kept = back.len();
        let waiting = entry(&mut self.effects, &taken.app);
        for line in back.into_iter().rev() {
            waiting.push_front(line);
        }
        kept
    }

    /// Records what came of one of `app`'s effects, or a notice for it (ticket
    /// [`NOTICE_TICKET`]): one waits at a time, as it says only that something
    /// changed, so an app that was napping is told once on waking. A line the
    /// shell gave back that still waits is withdrawn: what came of it is said,
    /// so it never runs after (#938: given back, then told it did not run).
    /// Only that line, the app's oldest with the ticket, as the back put it
    /// first: a line the app queued since under the same number (it
    /// reloaded) stays.
    pub fn finish(&mut self, app: &str, done: Done) {
        self.taken
            .retain(|(a, ticket, _)| !(a == app && *ticket == done.ticket));
        let given = self
            .given_back
            .iter()
            .position(|(a, ticket)| a == app && *ticket == done.ticket);
        if let Some(given) = given {
            self.given_back.remove(given);
            if let Some((_, waiting)) = self.effects.iter_mut().find(|(a, _)| a == app)
                && let Some(at) = waiting
                    .iter()
                    .position(|(_, line)| ticket_of(line) == Some(done.ticket))
            {
                waiting.remove(at);
            }
        }
        let results = entry(&mut self.results, app);
        if done.ticket == NOTICE_TICKET && results.iter().any(|d| *d == done) {
            return;
        }
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

/// Splits the notices off `results`: the notice the OS sent (`Ok` with ticket
/// [`NOTICE_TICKET`]; the first, however many came), and the rest, which answer
/// effects. A refusal under the notice's ticket is dropped: only the shell sends
/// a notice, and an app that queues a line with ticket 0 gets such a refusal for
/// it, which must not read as a notice.
pub fn split_notice(results: Vec<Done>) -> (Vec<Done>, Option<String>) {
    let mut notice = None;
    let mut rest = Vec::new();
    for done in results {
        match (done.ticket == NOTICE_TICKET, done.outcome) {
            (false, outcome) => rest.push(Done {
                ticket: done.ticket,
                outcome,
            }),
            (true, Ok(text)) => {
                notice.get_or_insert(text);
            }
            (true, Err(_)) => {}
        }
    }
    (rest, notice)
}

/// The effect the Terminal's Stop asks for ([`split_stops`]).
pub const SHELL_STOP: &str = "shell_stop";
/// The app whose Stop `desktop` takes: the image's Terminal.
pub const STOPS_FROM: &str = "terminal-window";

/// `lines` with their `shell_stop` calls taken out: those calls' tickets,
/// and the other lines, for the queue. `desktop` takes these from
/// [`STOPS_FROM`] alone, on a person's key or click, and interrupts the
/// shell's line at once: queued, a Stop would wait behind the line it stops.
pub fn split_stops(lines: &str) -> (Vec<u32>, String) {
    let stop = format!("call({{name: \"{SHELL_STOP}\", ");
    let mut tickets = Vec::new();
    let mut rest = String::new();
    for line in lines.lines() {
        let wave = line.strip_prefix("effect ").and_then(|l| l.split_once(' '));
        match (ticket_of(line), wave) {
            (Some(ticket), Some((_, wave))) if wave.starts_with(&stop) => tickets.push(ticket),
            _ => {
                rest.push_str(line);
                rest.push('\n');
            }
        }
    }
    (tickets, rest)
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

/// The shell's [`BACK`] request for `taken`.
pub fn encode_back(taken: &Taken) -> String {
    format!("{BACK}{}", encode_take(Some(taken)))
}

/// [`encode_back`] read back.
pub fn parse_back(text: &str) -> Option<Taken> {
    let (app, user, lines) = parse_take(text.strip_prefix(BACK)?)??;
    Some(Taken {
        app: app.to_string(),
        user,
        lines: lines.lines().map(String::from).collect(),
    })
}

/// The apps a [`TAKE_BUT`] request skips, or `None` for another request.
pub fn parse_take_but(text: &str) -> Option<Vec<&str>> {
    Some(text.strip_prefix(TAKE_BUT)?.split_whitespace().collect())
}

/// The shell's [`TAKE_BUT`] request skipping `apps`: a plain [`TAKE`] when
/// none.
pub fn encode_take_but(apps: &[&str]) -> String {
    match apps.is_empty() {
        true => String::from(TAKE),
        false => format!("{TAKE_BUT}{}", apps.join(" ")),
    }
}

/// The shell's take while it runs a line, skipping `apps`: a plain
/// [`TAKE_DURING`] when none.
pub fn encode_take_during_but(apps: &[&str]) -> String {
    match apps.is_empty() {
        true => String::from(TAKE_DURING),
        false => format!("{TAKE_DURING_BUT}{}", apps.join(" ")),
    }
}

/// The apps a take during a line skips ([`encode_take_during_but`]), or
/// `None` for another request.
pub fn parse_take_during_but(text: &str) -> Option<Vec<&str>> {
    match text == TAKE_DURING {
        true => Some(Vec::new()),
        false => Some(
            text.strip_prefix(TAKE_DURING_BUT)?
                .split_whitespace()
                .collect(),
        ),
    }
}

/// The shell's [`TAKE_APP`] request for `app`'s lines.
pub fn encode_take_app(app: &str) -> String {
    format!("{TAKE_APP}{app}")
}

/// The app a [`TAKE_APP`] request names, or `None` for another request.
pub fn parse_take_app(text: &str) -> Option<&str> {
    let app = text.strip_prefix(TAKE_APP)?;
    (!app.is_empty() && !app.contains(char::is_whitespace)).then_some(app)
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
#[path = "effects_tests.rs"]
mod tests;
