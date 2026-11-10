//! An app's network asks on their way to the shell and back (P5 task 13),
//! carried by `desktop` as Settings' requests are: `comphost` has no route to
//! the shell, and the app is `desktop`'s record of the caller (the kernel's
//! name for its process), never anything in a request. One request a call,
//! as text:
//!
//! - from an app's `comphost`, `net ask <host>\n<ceiling>` → the shell's
//!   answer for that app and host if one came since the last ask (taken), or
//!   `wait`, and the ask is put in line for the shell (once);
//! - from the shell, `net take` → `none`, or `ask <app> <host>\n<ceiling>`,
//!   the oldest;
//! - from the shell, `net done <app> <host>\n<answer>` → `ok`.
//!
//! The answer is the shell's `NetConnect::wire`: `granted <netdest>`, `asked`
//! (the person is asked on the card: ask again) or `refused <why>`. The
//! ceiling is `comphost`'s word for the destination its plan line names; the
//! kernel holds it to that, since `comphost` narrows the handle the plan gave
//! it, so a ceiling that says more reaches nothing more.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};

pub const ASK: &str = "net ask ";
pub const TAKE: &str = "net take";
pub const DONE: &str = "net done ";
/// The reply while the shell has not answered.
pub const WAIT: &str = "wait";
/// Asks that may wait for the shell, every app together.
pub const MAX_WAITING: usize = 16;
/// Answers that may wait for their app; past it the oldest goes, and its app
/// asks again.
pub const MAX_ANSWERS: usize = 16;
/// The longest host name.
pub const MAX_HOST: usize = 253;
/// The longest ceiling or answer.
pub const MAX_TEXT: usize = 1024;

/// One app's ask about one host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub app: String,
    pub host: String,
    pub ceiling: String,
}

/// Whether `text` is one printable line within `max` bytes.
fn line(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
}

/// Whether `host` may be a DNS name: lower-case letters, digits, `-` and `.`.
fn host_word(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
}

/// The asks waiting for the shell and the answers waiting for their apps.
#[derive(Default)]
pub struct Queue {
    waiting: VecDeque<Ask>,
    answers: VecDeque<(String, String, String)>,
}

impl Queue {
    /// `caller`'s ask (the text after [`ASK`]): the shell's answer if one
    /// waits, else `wait` with the ask in line, or why it is refused.
    pub fn ask(&mut self, caller: &str, text: &str) -> String {
        let Some((host, ceiling)) = text.split_once('\n') else {
            return String::from("refused the ask names no ceiling");
        };
        if !host_word(host) || !line(ceiling, MAX_TEXT) {
            return String::from("refused the ask is out of shape");
        }
        let mine = |(app, name, _): &(String, String, String)| app == caller && name == host;
        if let Some(at) = self.answers.iter().position(mine) {
            return self
                .answers
                .remove(at)
                .map(|(_, _, a)| a)
                .unwrap_or_default();
        }
        let same = |a: &Ask| a.app == caller && a.host == host;
        if self.waiting.iter().any(same) {
            return String::from(WAIT);
        }
        if self.waiting.len() >= MAX_WAITING {
            return String::from("refused too many network asks wait for the shell");
        }
        self.waiting.push_back(Ask {
            app: caller.to_string(),
            host: host.to_string(),
            ceiling: ceiling.to_string(),
        });
        String::from(WAIT)
    }

    /// The oldest waiting ask.
    pub fn take(&mut self) -> Option<Ask> {
        self.waiting.pop_front()
    }

    /// Keeps the shell's answer (the text after [`DONE`]) for its app.
    pub fn done(&mut self, text: &str) -> Result<(), &'static str> {
        let (head, answer) = text.split_once('\n').ok_or("the answer names no app")?;
        let (app, host) = head.split_once(' ').ok_or("the answer names no host")?;
        if !line(app, MAX_HOST) || !host_word(host) || !line(answer, MAX_TEXT) {
            return Err("the answer is out of shape");
        }
        self.answers.retain(|(a, h, _)| !(a == app && h == host));
        if self.answers.len() == MAX_ANSWERS {
            self.answers.pop_front();
        }
        self.answers
            .push_back((app.to_string(), host.to_string(), answer.to_string()));
        Ok(())
    }
}

/// The request `comphost` sends for `host` under `ceiling`.
pub fn encode_ask(host: &str, ceiling: &str) -> String {
    format!("{ASK}{host}\n{ceiling}")
}

/// The answer to [`TAKE`].
pub fn encode_take(taken: Option<&Ask>) -> String {
    match taken {
        Some(ask) => format!("ask {} {}\n{}", ask.app, ask.host, ask.ceiling),
        None => String::from("none"),
    }
}

/// [`encode_take`] read back: `Some(None)` for none, `None` for anything else.
pub fn parse_take(text: &str) -> Option<Option<Ask>> {
    if text == "none" {
        return Some(None);
    }
    let (head, ceiling) = text.split_once('\n')?;
    let (app, host) = head.strip_prefix("ask ")?.split_once(' ')?;
    Some(Some(Ask {
        app: app.to_string(),
        host: host.to_string(),
        ceiling: ceiling.to_string(),
    }))
}

/// The shell's answer for `app` and `host`.
pub fn encode_done(app: &str, host: &str, answer: &str) -> String {
    format!("{DONE}{app} {host}\n{answer}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CEILING: &str = "proto:tcp,addrs:none,ports:443,hosts:kai.test";

    #[test]
    fn an_ask_waits_once_and_its_answer_is_taken_once() {
        let mut queue = Queue::default();
        assert_eq!(
            queue.ask("net-fixture", &format!("a.kai.test\n{CEILING}")),
            WAIT
        );
        assert_eq!(
            queue.ask("net-fixture", &format!("a.kai.test\n{CEILING}")),
            WAIT
        );
        let ask = queue.take().unwrap();
        assert_eq!(
            ask,
            Ask {
                app: "net-fixture".into(),
                host: "a.kai.test".into(),
                ceiling: CEILING.into(),
            }
        );
        assert_eq!(queue.take(), None, "asked once");
        assert_eq!(parse_take(&encode_take(Some(&ask))), Some(Some(ask)));
        assert_eq!(parse_take(&encode_take(None)), Some(None));
        let done = encode_done("net-fixture", "a.kai.test", "asked");
        queue.done(done.strip_prefix(DONE).unwrap()).unwrap();
        // Another app's ask for the host is not answered with it.
        assert_eq!(queue.ask("other", &format!("a.kai.test\n{CEILING}")), WAIT);
        assert_eq!(
            queue.ask("net-fixture", &format!("a.kai.test\n{CEILING}")),
            "asked"
        );
        // Taken: the next ask waits for the shell again.
        assert_eq!(
            queue.ask("net-fixture", &format!("a.kai.test\n{CEILING}")),
            WAIT
        );
    }

    #[test]
    fn a_newer_answer_replaces_an_older_one() {
        let mut queue = Queue::default();
        queue.done("app a.kai.test\nasked").unwrap();
        queue.done("app a.kai.test\ngranted x").unwrap();
        assert_eq!(
            queue.ask("app", &format!("a.kai.test\n{CEILING}")),
            "granted x"
        );
    }

    #[test]
    fn what_is_out_of_shape_is_refused_and_the_queues_are_bounded() {
        let mut queue = Queue::default();
        for bad in [
            "a.kai.test",
            "A.kai.test\nx",
            "a b\nx",
            "a.kai.test\n",
            "a.kai.test\nx\ty",
        ] {
            assert!(queue.ask("app", bad).starts_with("refused"), "{bad:?}");
        }
        assert!(queue.done("app a.kai.test").is_err());
        assert!(queue.done("app\nasked").is_err());
        for i in 0..MAX_WAITING {
            assert_eq!(queue.ask("app", &format!("h{i}.kai.test\nc")), WAIT);
        }
        assert!(queue.ask("app", "last.kai.test\nc").starts_with("refused"));
        for i in 0..=MAX_ANSWERS {
            queue.done(&format!("app h{i}.kai.test\nasked")).unwrap();
        }
        // The oldest answer went; its app's ask waits again.
        assert_eq!(queue.ask("app", "h0.kai.test\nc"), WAIT);
        assert_eq!(queue.ask("app", "h1.kai.test\nc"), "asked");
    }
}
