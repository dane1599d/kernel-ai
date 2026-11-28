//! `rules refused`: the rules files refused at start and kept aside
//! (`rules.refused`, then `rules.refused.2` ... [`crate::MAX_REFUSED_RULES`]),
//! listed with their number, the date each was kept and why it was refused;
//! and `rules refused remove <n>`, which asks, and on the person's typed `y`
//! puts copy `n` in the trash, freeing its name (#1604's review). Nothing is
//! lost unless the person asks for it: a refused file is only ever kept, and
//! only this line takes one away.
//!
//! The move is a log entry of its own, with the generation `fs` committed it
//! as, so `log` shows it and `undo` moves the copy back (#1655's review). The
//! undo has no redo: only this line, which asks, takes a copy away again.
//!
//! The question waits for the very next line at the console, and for two
//! minutes ([`CONFIRM_EXPIRY_NS`]). Any other line leaves it, the card's,
//! `net denied` and an agent's answer included. A `y` while a newer question
//! is open (a proposal, a tune trial, `undo`'s, the card's) or once it has
//! expired answers neither question, and says so. Each outcome is audited:
//! put in the trash, kept (`n`), left, not put in the trash.
//!
//! Only the person's own line at the terminal reaches it. It is not a
//! registry row and no Rhai function, so no model call, script, standing
//! instruction or goal names it, and the desk panel and app windows refuse
//! the remove (only the console answers its question, as with `undo`'s).

use crate::actions::CONFIRM_EXPIRY_NS;
use crate::actions::answers::yes_or_no;
use crate::actlog::{Change, Entry, Form, State};
use crate::rules::Rules;
use crate::undo::{Inverse, Refused};
use crate::{FsRefused, MAX_REFUSED_RULES, Reply, Shell, refused_rules_file};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The log's action word for the move, which a bare `undo` past its undo
/// names.
pub(crate) const REMOVE_ACTION: &str = "rules_refused_remove";

/// What a panel or an app window is told for a `rules refused remove` line.
pub(crate) const REMOVE_AT_TERMINAL: &str =
    "type rules refused remove in the terminal: only its own prompt answers the question";

/// Whether `line` is a `rules refused remove ...` line, which only the
/// console's prompt may answer.
pub(crate) fn is_remove_line(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words.next() == Some("rules")
        && words.next() == Some("refused")
        && words.next() == Some("remove")
}

impl Shell {
    /// `rules refused` or `rules refused remove <n>`; `None` for any other
    /// line, or when `rules` is a variable.
    pub(crate) fn refused_line(&mut self, input: &str) -> Option<Reply> {
        if self.scope.contains("rules") {
            return None;
        }
        let words: Vec<&str> = input.split_whitespace().collect();
        match words.as_slice() {
            ["rules", "refused"] => Some(self.list_refused()),
            ["rules", "refused", "remove", n] => Some(self.ask_remove_refused(n)),
            ["rules", "refused", "remove", ..] => {
                Some(Reply::err("usage: rules refused remove <number>"))
            }
            _ => None,
        }
    }

    /// The kept copies, one line each: number, date kept, name and why it
    /// was refused.
    fn list_refused(&self) -> Reply {
        let mut lines = Vec::new();
        for n in 1..=MAX_REFUSED_RULES {
            let name = refused_rules_file(n);
            match self.host.refused_rules_read(&name) {
                Ok(None) => {}
                Ok(Some((text, kept_ns))) => {
                    let why = refused_why(&text);
                    lines.push(format!("{n}  {}  {name}: {why}", date(kept_ns)));
                }
                Err(why) => lines.push(format!("{n}  {name}: could not be read ({why})")),
            }
        }
        if lines.is_empty() {
            return Reply::ok("no refused rules files are kept");
        }
        Reply::ok(format!(
            "Refused rules files kept (rules refused remove <n> puts one in the trash):\n{}",
            lines.join("\n")
        ))
    }

    /// Asks before copy `n` goes: the next typed `y` answers.
    fn ask_remove_refused(&mut self, n: &str) -> Reply {
        let Some(n) = n
            .trim_start_matches('#')
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_REFUSED_RULES).contains(n))
        else {
            return Reply::err(format!(
                "usage: rules refused remove <number>, 1 to {MAX_REFUSED_RULES}"
            ));
        };
        // A question open now is the card's (the line left the rest): this
        // one asks after it, so any open when the answer comes is newer.
        if self.card_for_desk.is_some() || self.desk_ask_waiting() {
            return Reply::err("answer the card's question first, then ask again");
        }
        let name = refused_rules_file(n);
        match self.host.refused_rules_read(&name) {
            Ok(Some((_, kept_ns))) => {
                self.refused_asking = Some((n, self.host.now_ns()));
                self.host
                    .audit(&format!("rules refused remove {n} asks: {name}"));
                Reply::ok(format!(
                    "Put refused rules file {n} ({name}, kept {}) in the trash? y/n",
                    date(kept_ns)
                ))
            }
            Ok(None) => Reply::err(format!(
                "no refused rules file {n} is kept; rules refused lists them"
            )),
            Err(why) => Reply::err(format!("{name} could not be read: {why}")),
        }
    }

    /// The person's `y` or `n` to the question about copy `n`, asked at
    /// `asked_ns`, which `eval_line` took for this line. `None` when the line
    /// is neither or names a variable: the question is then left. A `y` or
    /// `n` while a newer question is open, or past [`CONFIRM_EXPIRY_NS`],
    /// answers neither question, and says so.
    pub(crate) fn refused_answer(
        &mut self,
        (n, asked_ns): (usize, u64),
        input: &str,
    ) -> Option<Reply> {
        let name = refused_rules_file(n);
        let Some(yes) = yes_or_no(input).filter(|_| !self.scope.contains(input)) else {
            self.refused_left(Some((n, asked_ns)));
            return None;
        };
        let newer = self.pending.is_some()
            || self.tuner.proposing()
            || self.undo_asking.is_some()
            || self.card_for_desk.is_some()
            || self.desk_ask_waiting();
        if newer {
            self.host.audit(&format!(
                "rules refused remove {n} left: a newer question is open; {name} kept"
            ));
            return Some(Reply::err(format!(
                "That answered neither question: {name} is kept, and a newer question is open; answer it where it is asked"
            )));
        }
        if self.host.now_ns().saturating_sub(asked_ns) > CONFIRM_EXPIRY_NS {
            self.host.audit(&format!(
                "rules refused remove {n} left: it expired; {name} kept"
            ));
            return Some(Reply::err(format!(
                "The question expired: {name} is kept; rules refused remove {n} asks again"
            )));
        }
        if !yes {
            self.host
                .audit(&format!("rules refused remove {n}: {name} kept (n)"));
            return Some(Reply::ok(format!("Kept {name}")));
        }
        Some(match self.host.refused_rules_trash(&name) {
            Ok(generation) => {
                self.host.audit(&format!(
                    "rules refused remove {n}: {name} put in the trash (fs generation {generation})"
                ));
                self.log_refused_trash(n, &name, generation);
                Reply::ok(format!("Put {name} in the trash; undo puts it back"))
            }
            Err(why) => {
                self.host.audit(&format!(
                    "rules refused remove {n}: {name} not put in the trash: {why}"
                ));
                Reply::err(format!("{name} was not put in the trash: {why}"))
            }
        })
    }

    /// The question `asked`, if any, left by a line that is not its answer.
    pub(crate) fn refused_left(&mut self, asked: Option<(usize, u64)>) {
        if let Some((n, _)) = asked {
            self.host.audit(&format!(
                "rules refused remove {n} left: another line came first; {} kept",
                refused_rules_file(n)
            ));
        }
    }

    /// The move of copy `n` to the trash, as `fs` committed it, logged so
    /// `log` shows it and `undo` puts it back.
    fn log_refused_trash(&mut self, n: usize, name: &str, generation: u64) {
        let id = self.next_id;
        self.next_id += 1;
        let line = format!("rules refused remove {n}");
        self.log.push(Entry {
            agent: agentid::Name::kai(),
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: None,
            pack: None,
            consent: String::from("typed"),
            state: State::Done,
            request: line.clone(),
            script: line,
            outcome: format!("{name} put in the trash"),
            changes: alloc::vec![Change {
                action: String::from(REMOVE_ACTION),
                owner: String::from("fs"),
                res: format!("/system/state/{name}"),
                before: String::from("kept"),
                after: format!("in the trash (fs generation {generation})"),
                inverse: Some(Inverse::Refused { n: n as u64 }),
            }],
        });
        self.save_log();
    }

    /// `undo` of `rules refused remove <n>`: copy `n` back from the trash.
    pub(crate) fn refused_untrash(&mut self, n: u64) -> Result<String, Refused> {
        let n = usize::try_from(n)
            .ok()
            .filter(|n| (1..=MAX_REFUSED_RULES).contains(n))
            .ok_or_else(|| Refused::Kept(format!("no refused rules file {n} is kept")))?;
        let name = refused_rules_file(n);
        match self.host.refused_rules_untrash(&name) {
            Ok(()) => {
                self.host
                    .audit(&format!("undo of rules refused remove {n}: {name} back"));
                Ok(format!("{name} back from the trash"))
            }
            Err(FsRefused::Failed(why)) => Err(Refused::Failed(why)),
            Err(FsRefused::Kept(why)) => Err(Refused::Kept(why)),
        }
    }
}

/// `undo` of `rules refused remove` on `/system/state`'s `Dir`, through
/// `ask`: Begin, TrashRestore and Commit as one action of its own. Every
/// failure after Begin sends Abort, a refused restore and a call that got
/// no answer alike: an action left open would answer each later Begin of
/// the shell's with Busy until `fs` restarted (#1673's review).
/// [`FsRefused::Kept`] when no try would do better: another copy holds the
/// name, or the trash holds none.
pub fn untrash_on(
    name: &str,
    action: u64,
    mut ask: impl FnMut(fsproto::Request) -> Result<fsproto::Reply, String>,
) -> Result<(), FsRefused> {
    use fsproto::{Error, Reply, Request};
    let answered = |reply: Reply| match reply {
        Reply::Failed(error) => format!("{error:?}"),
        other => format!("the fs service answered {other:?}"),
    };
    match ask(Request::Begin { action }) {
        Ok(Reply::Done) => {}
        Ok(other) => return Err(FsRefused::Failed(answered(other))),
        Err(why) => return Err(FsRefused::Failed(why)),
    }
    let why = match ask(Request::TrashRestore { path: name.into() }) {
        Ok(Reply::Done) => match ask(Request::Commit) {
            Ok(Reply::Committed { .. }) => return Ok(()),
            Ok(other) => FsRefused::Failed(answered(other)),
            Err(why) => FsRefused::Failed(why),
        },
        Ok(Reply::Failed(Error::Exists)) => FsRefused::Kept(format!(
            "{name} stays in the trash: another refused rules file holds that name now"
        )),
        Ok(Reply::Failed(Error::NotFound)) => {
            FsRefused::Kept(format!("the trash holds no {name}: it was emptied since"))
        }
        Ok(other) => FsRefused::Failed(answered(other)),
        Err(why) => FsRefused::Failed(why),
    };
    let _ = ask(Request::Abort);
    Err(why)
}

/// Why the rules file `text` is refused, as at start; a file that reads now
/// (the shell changed since) says so.
fn refused_why(text: &str) -> String {
    match Rules::restore(text) {
        Err(why) => why,
        Ok(_) => String::from("it reads now; it was refused when it was kept"),
    }
}

/// `ns` since 1970 as "YYYY-MM-DD HH:MM UTC".
fn date(ns: u64) -> String {
    let s = ns / 1_000_000_000;
    let (y, m, d) = civil(s / 86_400);
    let day_s = s % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        day_s / 3_600,
        day_s % 3_600 / 60
    )
}

/// The date `days` after 1970-01-01 (Hinnant's `civil_from_days`).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + u64::from(m <= 2), m, d)
}
