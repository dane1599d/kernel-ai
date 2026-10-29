//! The Settings app's `kai:settings` requests on their way to the shell and
//! back (P4 task 7), carried by `desktop` as an app's effects are: `comphost`
//! has no route to the shell, and the app is `desktop`'s own record of the
//! caller (the kernel's name for its process), never anything in a request.
//! One request a call, as text:
//!
//! - from the app's `comphost`, `settings put\n<lines>` → `ok`, each line
//!   `<ticket>\t<user 0|1>\t<request>`; any caller but [`SETTINGS_APP`] gets
//!   `err not the settings app`;
//! - from it, `settings results` → its answers, oldest first, taken: each
//!   `<ticket>\tok|err\t<len>\n<text>`, only whole answers that fit one
//!   message;
//! - from the shell, `settings take` → `none`, or `app <name>\n<lines>` in the
//!   put form;
//! - from the shell, `settings done <name>\n<answers>` → `ok`.
//!
//! `user` is `comphost`'s word that a real key or click made the request; the
//! shell checks the app again and refuses a change without it. The ticket is
//! `comphost`'s own and comes back as it went.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const PUT: &str = "settings put\n";
pub const RESULTS: &str = "settings results";
pub const TAKE: &str = "settings take";
pub const DONE: &str = "settings done ";
/// The one process that may ask: the Settings app's `comphost` line, named
/// after its component as Notes' window is (`shellcore`'s `SETTINGS_APP`).
pub const SETTINGS_APP: &str = "settings-window";
/// Requests that may wait for the shell.
pub const MAX_WAITING: usize = 8;
/// Answers that may wait for `comphost`; the oldest go first.
pub const MAX_ANSWERS: usize = 32;
/// The longest request kept.
pub const MAX_REQUEST: usize = 512;
/// The longest answer text: a longer one is `err too long`, never cut, since
/// the app reads it as data.
pub const MAX_ANSWER: usize = 4096;

/// One request, as the shell takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub ticket: u32,
    pub user: bool,
    pub text: String,
}

/// What one request came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub ticket: u32,
    pub outcome: Result<String, String>,
}

impl Answer {
    /// `outcome` for `ticket`, refused as too long past [`MAX_ANSWER`].
    pub fn new(ticket: u32, outcome: Result<String, String>) -> Self {
        let outcome = match outcome {
            Ok(text) | Err(text) if text.len() > MAX_ANSWER => Err(String::from("too long")),
            other => other,
        };
        Self { ticket, outcome }
    }

    fn encode(&self) -> String {
        let (word, text) = match &self.outcome {
            Ok(text) => ("ok", text),
            Err(text) => ("err", text),
        };
        format!("{}\t{word}\t{}\n{text}", self.ticket, text.len())
    }
}

/// The waiting requests and answers.
#[derive(Default)]
pub struct Queue {
    waiting: VecDeque<Request>,
    answers: VecDeque<Answer>,
}

impl Queue {
    /// Queues `caller`'s request lines, or refuses a caller that is not the
    /// Settings app. A line out of shape, too long or past [`MAX_WAITING`] is
    /// answered with an error at once, if it has a ticket to answer.
    pub fn put(&mut self, caller: &str, lines: &str) -> Result<(), &'static str> {
        if caller != SETTINGS_APP {
            return Err("not the settings app");
        }
        for line in lines.lines() {
            let refused = match parse_request(line) {
                Some(request) if self.waiting.len() < MAX_WAITING => {
                    self.waiting.push_back(request);
                    continue;
                }
                Some(request) => Answer::new(request.ticket, Err("too many waiting".into())),
                None => match ticket_of(line) {
                    Some(ticket) => Answer::new(ticket, Err("not a settings request".into())),
                    None => continue,
                },
            };
            self.finish(refused);
        }
        Ok(())
    }

    /// Every waiting request, oldest first, at most `budget` bytes in the put
    /// form (always at least one); `None` when none waits.
    pub fn take(&mut self, budget: usize) -> Option<Vec<Request>> {
        let mut taken = Vec::new();
        let mut used = 0;
        while let Some(request) = self.waiting.front() {
            let len = encode_request(request).len() + 1;
            if !taken.is_empty() && used + len > budget {
                break;
            }
            used += len;
            taken.extend(self.waiting.pop_front());
        }
        (!taken.is_empty()).then_some(taken)
    }

    /// Keeps what one request came to.
    pub fn finish(&mut self, answer: Answer) {
        if self.answers.len() == MAX_ANSWERS {
            self.answers.pop_front();
        }
        self.answers.push_back(answer);
    }

    /// The answers, oldest first, as [`RESULTS`] sends them: only whole ones
    /// within `budget` bytes, the rest left waiting.
    pub fn results(&mut self, budget: usize) -> String {
        let mut out = String::new();
        while let Some(answer) = self.answers.front() {
            let text = answer.encode();
            if out.len() + text.len() > budget {
                break;
            }
            out.push_str(&text);
            self.answers.pop_front();
        }
        out
    }
}

/// A request line in the put form.
pub fn encode_request(request: &Request) -> String {
    format!(
        "{}\t{}\t{}",
        request.ticket,
        u8::from(request.user),
        request.text
    )
}

/// [`encode_request`] read back; `None` for a line out of shape.
pub fn parse_request(line: &str) -> Option<Request> {
    let mut fields = line.splitn(3, '\t');
    let ticket = fields.next()?.parse().ok()?;
    let user = match fields.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let text = fields.next()?;
    let fits = !text.is_empty() && text.len() <= MAX_REQUEST;
    (fits && !text.contains(|c: char| c.is_control())).then(|| Request {
        ticket,
        user,
        text: text.to_string(),
    })
}

fn ticket_of(line: &str) -> Option<u32> {
    line.split('\t').next()?.parse().ok()
}

/// The answer to [`TAKE`].
pub fn encode_take(taken: Option<&[Request]>) -> String {
    match taken {
        Some(requests) => {
            let lines: Vec<String> = requests.iter().map(encode_request).collect();
            format!("app {SETTINGS_APP}\n{}", lines.join("\n"))
        }
        None => String::from("none"),
    }
}

/// [`encode_take`] read back: the app and its requests, `Some(None)` for
/// none, `None` for anything else.
pub fn parse_take(text: &str) -> Option<Option<(&str, Vec<Request>)>> {
    if text == "none" {
        return Some(None);
    }
    let (head, lines) = text.split_once('\n')?;
    let app = head.strip_prefix("app ")?;
    let requests = lines
        .lines()
        .map(parse_request)
        .collect::<Option<Vec<_>>>()?;
    Some(Some((app, requests)))
}

/// The shell's [`DONE`] request for `answers`.
pub fn encode_done(app: &str, answers: &[Answer]) -> String {
    let body: String = answers.iter().map(Answer::encode).collect();
    format!("{DONE}{app}\n{body}")
}

/// Answers as [`RESULTS`] and [`DONE`] carry them; `None` for any out of
/// shape, so a cut message is never read as a shorter answer.
pub fn parse_answers(mut text: &str) -> Option<Vec<Answer>> {
    let mut answers = Vec::new();
    while !text.is_empty() {
        let (head, rest) = text.split_once('\n')?;
        let mut fields = head.split('\t');
        let ticket = fields.next()?.parse().ok()?;
        let ok = match fields.next()? {
            "ok" => true,
            "err" => false,
            _ => return None,
        };
        let len: usize = fields.next()?.parse().ok()?;
        if fields.next().is_some() || len > MAX_ANSWER {
            return None;
        }
        let body = rest.get(..len)?.to_string();
        text = &rest[len..];
        answers.push(Answer {
            ticket,
            outcome: if ok { Ok(body) } else { Err(body) },
        });
    }
    Some(answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(ticket: u32, user: bool, text: &str) -> Request {
        Request {
            ticket,
            user,
            text: text.into(),
        }
    }

    #[test]
    fn only_the_settings_app_may_put() {
        let mut queue = Queue::default();
        assert_eq!(
            queue.put("notes", "1\t1\trules"),
            Err("not the settings app")
        );
        assert_eq!(queue.take(8192), None);
        assert_eq!(queue.put(SETTINGS_APP, "1\t1\trules"), Ok(()));
        assert_eq!(
            queue.take(8192),
            Some(alloc::vec![request(1, true, "rules")])
        );
    }

    #[test]
    fn a_request_and_its_answer_go_round_whole() {
        let mut queue = Queue::default();
        queue
            .put(SETTINGS_APP, "7\t0\trules\n8\t1\tkeep-running on notes")
            .unwrap();
        let taken = queue.take(8192);
        let wire = encode_take(taken.as_deref());
        let (app, requests) = parse_take(&wire).unwrap().unwrap();
        assert_eq!(app, SETTINGS_APP);
        assert_eq!(requests.len(), 2);
        let answers = [
            Answer::new(7, Ok("#1 set_policy\nS1 on idle".into())),
            Answer::new(8, Err("a change needs a key or a click".into())),
        ];
        let done = encode_done(app, &answers);
        let body = done.strip_prefix(&format!("{DONE}{app}\n")).unwrap();
        for answer in parse_answers(body).unwrap() {
            queue.finish(answer);
        }
        assert_eq!(parse_answers(&queue.results(8192)), Some(answers.to_vec()));
        assert_eq!(queue.results(8192), "");
    }

    #[test]
    fn a_long_answer_is_refused_never_cut() {
        let long = "x".repeat(MAX_ANSWER + 1);
        assert_eq!(Answer::new(1, Ok(long)).outcome, Err("too long".into()));
        let max = "x".repeat(MAX_ANSWER);
        assert!(Answer::new(1, Ok(max)).outcome.is_ok());
    }

    #[test]
    fn results_send_only_whole_answers_and_keep_the_rest() {
        let mut queue = Queue::default();
        queue.finish(Answer::new(1, Ok("a".repeat(3000))));
        queue.finish(Answer::new(2, Ok("b".repeat(3000))));
        let first = parse_answers(&queue.results(4000)).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].ticket, 1);
        let second = parse_answers(&queue.results(4000)).unwrap();
        assert_eq!(second[0].ticket, 2);
    }

    #[test]
    fn bad_lines_are_answered_or_dropped_and_the_queue_is_bounded() {
        let mut queue = Queue::default();
        let lines = "1\t2\trules\nnot a line\n3\t1\t\n4\t1\ta\u{7}b";
        queue.put(SETTINGS_APP, lines).unwrap();
        assert_eq!(queue.take(8192), None);
        let answers = parse_answers(&queue.results(8192)).unwrap();
        let tickets: Vec<u32> = answers.iter().map(|a| a.ticket).collect();
        assert_eq!(tickets, [1, 3, 4]);
        let many: Vec<String> = (0..=MAX_WAITING as u32)
            .map(|t| format!("{t}\t0\trules"))
            .collect();
        queue.put(SETTINGS_APP, &many.join("\n")).unwrap();
        assert_eq!(queue.take(8192).map(|t| t.len()), Some(MAX_WAITING));
        let refused = parse_answers(&queue.results(8192)).unwrap();
        assert_eq!(refused[0].outcome, Err("too many waiting".into()));
    }

    #[test]
    fn a_cut_or_forged_answer_reads_as_nothing() {
        assert_eq!(parse_answers("1\tok\t5\nabc"), None);
        assert_eq!(parse_answers("1\tmaybe\t1\na"), None);
        assert_eq!(parse_answers("1\tok\t1\textra\na"), None);
        assert_eq!(parse_answers(""), Some(Vec::new()));
    }
}
