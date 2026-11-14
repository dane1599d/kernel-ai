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
//! A kept answer lives [`KEEP_MS`] at most: `comphost` asks again every
//! 500 ms, so a fresh answer is taken well within it, and one left over (its
//! app gone, or a grant since revoked in Settings) is dropped rather than
//! handed out later: an old `granted` never gets past a newer refusal.
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
/// How long the shell's answer is kept for its app: four of `comphost`'s
/// asks (one every 500 ms). Past it the answer is dropped and the app's next
/// ask goes to the shell again.
pub const KEEP_MS: u64 = 4 * 500;

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

/// The shell's answer for one app and host, and when it came.
struct Kept {
    app: String,
    host: String,
    answer: String,
    at_ms: u64,
}

/// The asks waiting for the shell and the answers waiting for their apps.
#[derive(Default)]
pub struct Queue {
    waiting: VecDeque<Ask>,
    answers: VecDeque<Kept>,
}

impl Queue {
    /// `caller`'s ask (the text after [`ASK`]) at `now_ms`: the shell's
    /// answer if one waits and is under [`KEEP_MS`] old, else `wait` with the
    /// ask in line, or why it is refused.
    pub fn ask(&mut self, caller: &str, text: &str, now_ms: u64) -> String {
        let Some((host, ceiling)) = text.split_once('\n') else {
            return String::from("refused the ask names no ceiling");
        };
        if !host_word(host) || !line(ceiling, MAX_TEXT) {
            return String::from("refused the ask is out of shape");
        }
        self.forget_old(now_ms);
        let mine = |k: &Kept| k.app == caller && k.host == host;
        if let Some(kept) = self
            .answers
            .iter()
            .position(mine)
            .and_then(|at| self.answers.remove(at))
        {
            return kept.answer;
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

    /// Keeps the shell's answer (the text after [`DONE`]) for its app, from
    /// `now_ms` for [`KEEP_MS`].
    pub fn done(&mut self, text: &str, now_ms: u64) -> Result<(), &'static str> {
        let (head, answer) = text.split_once('\n').ok_or("the answer names no app")?;
        let (app, host) = head.split_once(' ').ok_or("the answer names no host")?;
        if !line(app, MAX_HOST) || !host_word(host) || !line(answer, MAX_TEXT) {
            return Err("the answer is out of shape");
        }
        self.forget_old(now_ms);
        self.answers.retain(|k| !(k.app == app && k.host == host));
        if self.answers.len() == MAX_ANSWERS {
            self.answers.pop_front();
        }
        self.answers.push_back(Kept {
            app: app.to_string(),
            host: host.to_string(),
            answer: answer.to_string(),
            at_ms: now_ms,
        });
        Ok(())
    }

    /// Drops every kept answer [`KEEP_MS`] old or older at `now_ms`. One
    /// from a clock that seems to have gone back is dropped too: its age is
    /// unknown.
    fn forget_old(&mut self, now_ms: u64) {
        self.answers
            .retain(|k| now_ms >= k.at_ms && now_ms - k.at_ms < KEEP_MS);
    }
}

/// The destination one call to `fetch` is made on, once the shell has
/// granted `host`: that host alone, exactly, on 443 (the audit of tasks 12
/// and 13). Never the app's whole list, so a redirect within a call reaches
/// no other host, granted or not: it comes back to be asked about by name.
pub fn call_dest(host: &str) -> String {
    format!("proto:tcp,addrs:none,ports:443,hosts:{host},match:exact")
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
            queue.ask("net-fixture", &format!("a.kai.test\n{CEILING}"), 0),
            WAIT
        );
        assert_eq!(
            queue.ask("net-fixture", &format!("a.kai.test\n{CEILING}"), 0),
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
        queue.done(done.strip_prefix(DONE).unwrap(), 0).unwrap();
        // Another app's ask for the host is not answered with it.
        assert_eq!(
            queue.ask("other", &format!("a.kai.test\n{CEILING}"), 0),
            WAIT
        );
        assert_eq!(
            queue.ask("net-fixture", &format!("a.kai.test\n{CEILING}"), 0),
            "asked"
        );
        // Taken: the next ask waits for the shell again.
        assert_eq!(
            queue.ask("net-fixture", &format!("a.kai.test\n{CEILING}"), 0),
            WAIT
        );
    }

    #[test]
    fn a_calls_destination_is_its_one_host_and_no_other() {
        let dest = netdest::Dest::parse(&call_dest("updates.kai.test")).unwrap();
        assert!(dest.exact);
        assert!(dest.allows_name("updates.kai.test"));
        // Another host the app holds a grant for is out of reach in this call,
        // as is a name under the host.
        for other in ["assets.kai.test", "a.updates.kai.test", "kai.test"] {
            assert!(!dest.allows_name(other), "{other}");
        }
        let ceiling = netdest::Dest::parse(CEILING).unwrap();
        assert!(dest.within(&ceiling));
    }

    #[test]
    fn a_newer_answer_replaces_an_older_one() {
        let mut queue = Queue::default();
        queue.done("app a.kai.test\nasked", 0).unwrap();
        queue.done("app a.kai.test\ngranted x", 0).unwrap();
        assert_eq!(
            queue.ask("app", &format!("a.kai.test\n{CEILING}"), 0),
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
            assert!(queue.ask("app", bad, 0).starts_with("refused"), "{bad:?}");
        }
        assert!(queue.done("app a.kai.test", 0).is_err());
        assert!(queue.done("app\nasked", 0).is_err());
        for i in 0..MAX_WAITING {
            assert_eq!(queue.ask("app", &format!("h{i}.kai.test\nc"), 0), WAIT);
        }
        assert!(
            queue
                .ask("app", "last.kai.test\nc", 0)
                .starts_with("refused")
        );
        for i in 0..=MAX_ANSWERS {
            queue.done(&format!("app h{i}.kai.test\nasked"), 0).unwrap();
        }
        // The oldest answer went; its app's ask waits again.
        assert_eq!(queue.ask("app", "h0.kai.test\nc", 0), WAIT);
        assert_eq!(queue.ask("app", "h1.kai.test\nc", 0), "asked");
    }

    #[test]
    fn a_kept_answer_is_handed_out_within_its_lifetime() {
        let mut queue = Queue::default();
        queue.done("app a.kai.test\ngranted x", 1_000).unwrap();
        assert_eq!(
            queue.ask(
                "app",
                &format!("a.kai.test\n{CEILING}"),
                1_000 + KEEP_MS - 1
            ),
            "granted x"
        );
    }

    #[test]
    fn an_expired_grant_is_never_handed_out_and_the_shell_is_asked_again() {
        let mut queue = Queue::default();
        queue.done("app a.kai.test\ngranted x", 1_000).unwrap();
        // Settings revokes the grant meanwhile; the app's next ask comes late.
        assert_eq!(
            queue.ask("app", &format!("a.kai.test\n{CEILING}"), 1_000 + KEEP_MS),
            WAIT
        );
        let ask = queue.take().unwrap();
        assert_eq!((ask.app.as_str(), ask.host.as_str()), ("app", "a.kai.test"));
        // The shell's fresh answer is the one used.
        queue
            .done("app a.kai.test\nrefused revoked", 1_000 + KEEP_MS + 10)
            .unwrap();
        assert_eq!(
            queue.ask(
                "app",
                &format!("a.kai.test\n{CEILING}"),
                1_000 + KEEP_MS + 20
            ),
            "refused revoked"
        );
    }

    #[test]
    fn a_kept_answer_stamped_after_now_is_not_used() {
        let mut queue = Queue::default();
        queue.done("app a.kai.test\ngranted x", 5_000).unwrap();
        assert_eq!(
            queue.ask("app", &format!("a.kai.test\n{CEILING}"), 4_999),
            WAIT
        );
    }
}
