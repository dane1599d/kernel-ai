//! `power_reboot` (founder test 2, backlog item 13): the person restarts the
//! computer from inside the OS. Only their own line reaches it: `restart`,
//! `reboot` or one of a few plain phrasings ([`is_restart_line`]), caught before
//! Rhai and the model. A question ("how do I restart?") is none of them, and the
//! row is in no pack, so no model call and no script restarts.
//!
//! It always asks, with no standing rule on offer: a restart stops every program
//! and nothing undoes it. On an Allow whose ONCE checks, the shell logs the
//! restart, saves its log, and calls the kernel ([`crate::Host::power_reboot`]).
//! `fs` commits every change it has answered before it answers, so what was
//! saved is on the disk; an app's open action is lost as a power cut loses it.

use crate::actions::{How, Pending};
use crate::actlog::{Change, Entry, State};
use crate::script::Checked;
use crate::{Reply, Shell};
use agentabi::consent::Why;
use alloc::format;
use alloc::string::String;

/// What the question says the restart does, before the call.
const SUBJECT: &str = "Restart the computer: every program stops, and nothing undoes it";

/// The lines that restart, after [`plain`]: a bare word, or the word and what
/// it restarts.
const LINES: &[&str] = &[
    "restart",
    "reboot",
    "restart now",
    "reboot now",
    "restart the computer",
    "reboot the computer",
    "restart my computer",
    "reboot my computer",
    "restart the machine",
    "reboot the machine",
    "restart the pc",
    "reboot the pc",
    "restart the system",
    "reboot the system",
    "restart kai",
    "reboot kai",
];

/// `text` lower-cased, with a leading "please" or "kai,", a trailing ", kai",
/// "please" or full stop or "!" taken off. `None` for a question.
fn plain(text: &str) -> Option<String> {
    let text = text.trim().to_ascii_lowercase();
    if text.contains('?') {
        return None;
    }
    let mut text = text.trim_end_matches(['.', '!']).trim();
    for prefix in ["kai, ", "kai ", "please "] {
        text = text.strip_prefix(prefix).unwrap_or(text).trim();
    }
    for suffix in [", kai", ", please", " please"] {
        text = text.strip_suffix(suffix).unwrap_or(text).trim();
    }
    Some(String::from(text))
}

/// Whether `text` is the person asking to restart the computer.
pub(crate) fn is_restart_line(text: &str) -> bool {
    plain(text).is_some_and(|line| LINES.contains(&line.as_str()))
}

impl Shell {
    /// A restart line: asks once, with what it does, and waits for the answer.
    pub(crate) fn power_line(&mut self, request: &str) -> Reply {
        let id = self.next_id;
        self.next_id += 1;
        let call = power_call();
        self.host.audit(&format!("request {id}: {call} asks"));
        let checked = Checked::shown_only(&call);
        let subject = Some(String::from(SUBJECT));
        let mut reply = self.ask_call(id, request, checked, Why::NoUndo, None, subject);
        if reply.confirm.is_some() && reply.text.starts_with("Run ") {
            reply.text = format!("{SUBJECT}. {}", reply.text);
        }
        if let Some(pending) = &mut self.pending {
            pending.power = true;
        }
        reply
    }

    /// The answer to the restart's question: an Allow whose ONCE checks logs
    /// the restart, saves the log and restarts. The reply comes back only when
    /// the kernel did not restart.
    pub(crate) fn answer_power(&mut self, pending: Pending, text: &str) -> Reply {
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        let id = pending.id;
        let how = How {
            consent: String::from("yes"),
            ..pending.how.clone()
        };
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: how.form,
            of: None,
            tier: Some(how.tier),
            pack: None,
            consent: how.consent,
            state: State::Done,
            request: pending.request.clone(),
            script: pending.checked.script.clone(),
            outcome: String::from("ran"),
            changes: alloc::vec![Change {
                action: String::from("power_reboot"),
                owner: String::from("kernel"),
                res: String::from("machine"),
                before: String::new(),
                after: String::from("restart"),
                inverse: None,
            }],
        });
        self.save_log();
        self.remember(&pending, "Restarting the computer");
        self.host.audit(&format!("request {id}: restarting"));
        match self.host.power_reboot() {
            Ok(()) => Reply::ok("Restarting the computer"),
            Err(why) => Reply::err(format!("The computer did not restart: {why}")),
        }
    }
}

/// The canonical call the question shows and its Allow's ONCE names.
pub(crate) fn power_call() -> String {
    termcore::once::call_text(agentabi::POWER_REBOOT.name, &[])
        .unwrap_or_else(|| String::from("power_reboot()"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_restart_lines_are_plain_words() {
        for line in [
            "restart",
            "Reboot",
            "restart the computer",
            "please restart the computer",
            "restart now.",
            "Kai, reboot",
            "restart, kai",
            "reboot my computer please",
        ] {
            assert!(is_restart_line(line), "{line}");
        }
    }

    #[test]
    fn a_question_or_a_longer_line_is_no_restart() {
        for line in [
            "how do I restart?",
            "restart?",
            "how do i restart",
            "can you restart the computer?",
            "tell me how to restart the computer",
            "restart the terminal",
            "restart the notes app",
            "don't restart",
            "restarting",
            "restart restart",
        ] {
            assert!(!is_restart_line(line), "{line}");
        }
    }
}
