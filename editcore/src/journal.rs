//! The kernel's journal of the edit session (M9d), so an edit service that
//! restarts gets the founder's session back. It holds the open, then every
//! command the service answered `ok` except `build`, `try`, `help` and `status`,
//! each with an FNV-64 digest of the reply. An upload the store took for the
//! session's version leaves `#built <label>` (COMP_PUT tells it, [`Journal::built`]);
//! a failed build that moved the cursor leaves `#at <path>:<line>`, from the line
//! the next reply begins with ([`ipc::split_moved`]). A command that got no answer
//! is never recorded, so the one that killed the service is never replayed.
//!
//! The service replays the entries (`service/replay.rs`) and stops at the first
//! reply whose digest differs, dropping the session; the entries before it
//! matched once, so they are replayed again. A replay from the open replaces
//! any session the service holds: the journal is what the founder was answered.

use crate::ipc::{self, At, Ipc};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Past this many bytes the journal warns once and records nothing more.
pub const MAX_JOURNAL: usize = 128 * 1024;
/// Room a `replay <from>` header takes in a message.
const HEADER: usize = 24;
/// The longest command line an entry can hold: the entry is `<digest> cmd <line>`
/// and, with a replay's header, must fit one message. A longer one is answered
/// but cannot be journaled, which would switch the journal off.
pub const MAX_COMMAND: usize = kabi::MAX_MESSAGE - HEADER - (16 + 1 + 4);
/// Commands that reach the host or a component, or change nothing.
const SKIPPED: [&str; 5] = ["build", "try", "help", "status", "view"];
/// Commands that change the text.
const EDITS: [&str; 8] = ["s", "c", "i", "a", "d", "split", "join", "undo"];
/// How a fresh open's reply ends when it dropped another session's edits, which a
/// replay into a fresh service never does.
const DROPPED: &[u8] = b"; dropped the unbuilt edits to ";
/// Most of a reply a notice quotes.
const QUOTED: usize = 80;
/// Why the service stops a replay that does not start at the open and finds no
/// session: a newer service took the later messages.
pub(crate) const NO_SESSION: &str = "there is no session to go on with";

/// FNV-1a, 64 bits.
pub fn digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The digest an entry keeps of `reply` to `message`.
pub fn reply_digest(message: &[u8], reply: &[u8]) -> u64 {
    let dropped = reply.windows(DROPPED.len()).position(|w| w == DROPPED);
    match dropped {
        Some(at) if message.starts_with(b"open ") => digest(&reply[..at]),
        _ => digest(reply),
    }
}

/// An entry's digest and message.
pub fn parse_entry(entry: &str) -> Option<(u64, &str)> {
    let (digest, message) = entry.split_once(' ')?;
    let digest = u64::from_str_radix(digest, 16)
        .ok()
        .filter(|_| digest.len() == 16)?;
    Some((digest, message))
}

/// The start of `text`, cut for a notice.
pub fn quoted(text: &str) -> &str {
    let line = text.lines().next().unwrap_or("");
    &line[..line.floor_char_boundary(QUOTED)]
}

/// An entry's message, after its digest.
fn message(entry: &str) -> &str {
    entry.get(17..).unwrap_or("")
}

/// An entry's command word, when it holds a command.
fn word(entry: &str) -> Option<&str> {
    message(entry).strip_prefix("cmd ")?.split(' ').next()
}

/// An entry that is a note rather than a message: `#built ...` or `#at ...`.
fn note(text: &str) -> String {
    format!("{:016x} {text}", digest(text.as_bytes()))
}

/// A place a moved cursor names, `<path>:<line>`, as a replay can go to it.
fn place_ok(place: &str) -> bool {
    place
        .rsplit_once(':')
        .is_some_and(|(path, line)| !path.is_empty() && line.parse::<usize>().is_ok())
}

/// How a restore went: what to tell the founder, and the commands that did not
/// come back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restore {
    pub notice: String,
    pub lost: Vec<String>,
    /// The service did not answer; the journal is kept for one more try.
    pub again: bool,
}

/// Why a replay stopped.
enum Stop {
    /// Entry `at` did not replay, and the service dropped the session.
    At(usize, String),
    /// A message got no answer.
    Silent,
}

#[derive(Clone, Debug, Default)]
pub struct Journal {
    /// `<digest> <message>`, oldest first; the first is the open.
    entries: Vec<String>,
    bytes: usize,
    /// The component and version the open named.
    session: Option<(String, String)>,
    /// Over [`MAX_JOURNAL`]: nothing more is recorded.
    full: bool,
    /// The last replay got no answer.
    unanswered: bool,
}

impl Journal {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            bytes: 0,
            session: None,
            full: false,
            unanswered: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn session(&self) -> Option<(&str, &str)> {
        self.session.as_ref().map(|(c, l)| (c.as_str(), l.as_str()))
    }

    /// Commands recorded that change the text.
    pub fn edits(&self) -> usize {
        let edit = |entry: &&String| word(entry).is_some_and(|w| EDITS.contains(&w));
        self.entries.iter().filter(edit).count()
    }

    /// Records `message`, which the service answered `reply`; a warning, once,
    /// when the journal is full. A cursor a build moved before the message is
    /// recorded first, and the reply's digest leaves out the line that says so.
    pub fn record(&mut self, message: &[u8], reply: &[u8]) -> Option<String> {
        let (moved, reply) = ipc::split_moved(reply);
        let decoded = ipc::decode_ipc(message);
        let edits = matches!(decoded, Ok(Ipc::Open { .. } | Ipc::Cmd(_)));
        let mut warning = None;
        if let Some(place) = moved.filter(|place| edits && place_ok(place))
            && self.session.is_some()
        {
            warning = self.push(note(&format!("#at {place}")));
        }
        let entry = match decoded {
            Ok(Ipc::Open {
                component,
                label,
                at,
            }) => self.opened(component, label, at, reply),
            Ok(Ipc::Cmd(line)) if self.session.is_some() => self.command(line, reply),
            _ => None,
        };
        let pushed = entry.and_then(|entry| self.push(entry));
        warning.or(pushed)
    }

    /// An upload of `component` built from `base` was stored as `label`. The
    /// session counts as built from it when it is based there; a replay checks
    /// that the upload's source is the text at this point (`service/replay.rs`).
    pub fn built(&mut self, component: &str, base: &str, label: &str) -> Option<String> {
        if self.session() != Some((component, base)) {
            return None;
        }
        self.push(note(&format!("#built {label}")))
    }

    /// Adds an entry; a warning, once, when the journal is full.
    fn push(&mut self, entry: String) -> Option<String> {
        if self.full {
            return None;
        }
        let fits = self.bytes + entry.len() < MAX_JOURNAL
            && entry.len() + HEADER <= kabi::MAX_MESSAGE
            && !entry.contains('\n');
        if !fits {
            self.full = true;
            return Some(format!(
                "the session journal is full ({} KiB); if the edit service restarts, edits from here on are lost",
                MAX_JOURNAL / 1024
            ));
        }
        self.bytes += entry.len() + 1;
        self.entries.push(entry);
        None
    }

    /// A fresh open starts the journal again; one of the same version only
    /// resumed the session, and may have moved the cursor.
    fn opened(
        &mut self,
        component: &str,
        label: Option<&str>,
        at: Option<At<'_>>,
        reply: &[u8],
    ) -> Option<String> {
        let rest = core::str::from_utf8(reply.strip_prefix(b"ok editing ")?).ok()?;
        let mut words = rest.split(' ');
        let (name, opened) = (words.next()?, words.next()?.trim_end_matches([':', ',']));
        if name != component || label.is_some_and(|label| label != opened) {
            return None;
        }
        let message = ipc::encode_ipc(&Ipc::Open {
            component,
            label: Some(opened),
            at,
        });
        let message = String::from_utf8(message).ok()?;
        if self.session() != Some((component, opened)) {
            self.truncate(0);
            self.session = Some((component.to_string(), opened.to_string()));
        }
        let digest = reply_digest(message.as_bytes(), reply);
        Some(format!("{digest:016x} {message}"))
    }

    fn command(&mut self, line: &str, reply: &[u8]) -> Option<String> {
        let word = line.split(' ').next().unwrap_or("");
        let digest = digest(reply);
        let text = core::str::from_utf8(reply).ok()?;
        if !text.starts_with("ok ") || SKIPPED.contains(&word) {
            return None;
        }
        if word == "reset" {
            self.truncate(1);
        }
        Some(format!("{digest:016x} cmd {line}"))
    }

    /// Keeps the first `count` entries; with none, there is no session.
    fn truncate(&mut self, count: usize) {
        self.entries.truncate(count);
        self.bytes = self.entries.iter().map(|e| e.len() + 1).sum();
        self.full = false;
        if self.entries.is_empty() {
            self.session = None;
        }
    }

    /// The replay messages for the first `count` entries, each one IPC message.
    pub fn messages(&self, count: usize) -> Vec<Vec<u8>> {
        let count = count.min(self.len());
        let mut out = Vec::new();
        let mut from = 0;
        while from < count {
            // Each entry fits a message on its own; `record` sees to that.
            let mut to = from + 1;
            let mut size = HEADER + self.entries[from].len();
            while to < count && size + 1 + self.entries[to].len() <= kabi::MAX_MESSAGE {
                size += 1 + self.entries[to].len();
                to += 1;
            }
            let entries = self.entries[from..to].join("\n");
            out.push(ipc::encode_ipc(&Ipc::Replay {
                from,
                entries: &entries,
            }));
            from = to;
        }
        out
    }

    /// Replays the first `count` entries; where it stopped, and why.
    fn replay(
        &self,
        count: usize,
        send: &mut dyn FnMut(&[u8]) -> Option<Vec<u8>>,
    ) -> Result<(), Stop> {
        for message in self.messages(count) {
            let reply = send(&message).ok_or(Stop::Silent)?;
            let reply = String::from_utf8_lossy(&reply);
            if let Some(n) = reply.strip_prefix("ok replayed ")
                && n.parse::<usize>().is_ok()
            {
                continue;
            }
            let stopped = reply
                .strip_prefix("err replay stopped at ")
                .and_then(|rest| rest.split_once(": "))
                .and_then(|(at, why)| Some(Stop::At(at.parse().ok()?, quoted(why).into())));
            let other = || Stop::At(0, format!("it answered {:?}", quoted(&reply)));
            return Err(stopped.unwrap_or_else(other));
        }
        Ok(())
    }

    /// Brings a restarted edit service back to this session; `send` delivers one
    /// message and returns the reply, None when none came. The journal keeps what
    /// was restored.
    pub fn restore(&mut self, send: &mut dyn FnMut(&[u8]) -> Option<Vec<u8>>) -> Restore {
        let (component, label) = self.session.clone().unwrap_or_default();
        let lead = "the edit service restarted;";
        let mut stopped = self.replay(self.len(), send);
        // A newer service took over between messages: start again from the open.
        if matches!(&stopped, Err(Stop::At(_, why)) if why == NO_SESSION) {
            stopped = self.replay(self.len(), send);
        }
        let unanswered = core::mem::take(&mut self.unanswered);
        let (at, why) = match stopped {
            Ok(()) => {
                let full = match self.full {
                    true => "; edits after the journal filled are lost",
                    false => "",
                };
                let notice = format!(
                    "{lead} restored {component} {label} with {}{full}",
                    self.counted()
                );
                return Restore {
                    notice,
                    lost: Vec::new(),
                    again: false,
                };
            }
            // Once only: a replay that kills the service must not do so for ever.
            Err(Stop::Silent) if !unanswered => {
                self.unanswered = true;
                return Restore {
                    notice: format!(
                        "{lead} it did not answer the replay; the next line tries again"
                    ),
                    lost: Vec::new(),
                    again: true,
                };
            }
            Err(Stop::Silent) => (0, String::from("it did not answer the replay again")),
            Err(Stop::At(at, why)) => (at, why),
        };
        let all = self.commands();
        // The service dropped the session; the entries before `at` matched.
        let at = at.min(self.len());
        let kept = match at > 0 && self.replay(at, send).is_ok() {
            true => at,
            false => 0,
        };
        self.truncate(kept);
        let notice = match kept {
            0 => format!("{lead} could not restore {component} {label} ({why}); :journal lists it"),
            _ => format!(
                "{lead} restored {component} {label} with {}; the rest did not replay ({why}); :journal lists it",
                self.counted()
            ),
        };
        Restore {
            notice,
            lost: all[kept..].to_vec(),
            again: false,
        }
    }

    /// `1 edit`, `6 edits`.
    pub fn counted(&self) -> String {
        match self.edits() {
            1 => String::from("1 edit"),
            n => format!("{n} edits"),
        }
    }

    /// The entries as the founder would type them.
    pub fn commands(&self) -> Vec<String> {
        let shown = |entry: &String| {
            let message = message(entry);
            if let Some(line) = message.strip_prefix("cmd ") {
                line.to_string()
            } else if let Some(label) = message.strip_prefix("#built ") {
                format!("(built {label})")
            } else if let Some(place) = message.strip_prefix("#at ") {
                format!("(a build failed; the cursor went to {place})")
            } else if let Ok(Ipc::Open {
                component,
                label,
                at,
            }) = ipc::decode_ipc(message.as_bytes())
            {
                let open = format!(":edit {component} {}", label.unwrap_or(""));
                match at {
                    Some(at) => format!("{open} (at {}:{})", at.path, at.line),
                    None => open,
                }
            } else {
                message.to_string()
            }
        };
        self.entries.iter().map(shown).collect()
    }
}
