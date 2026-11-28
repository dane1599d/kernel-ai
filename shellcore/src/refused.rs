//! `rules refused`: the rules files refused at start and kept aside
//! (`rules.refused`, then `rules.refused.2` ... [`crate::MAX_REFUSED_RULES`]),
//! listed with their number, the date each was kept and why it was refused;
//! and `rules refused remove <n>`, which asks, and on the person's typed `y`
//! puts copy `n` in the trash, freeing its name (#1604's review). Nothing is
//! lost unless the person asks for it: a refused file is only ever kept, and
//! only this line takes one away.
//!
//! Only the person's own line at the terminal reaches it. It is not a
//! registry row and no Rhai function, so no model call, script, standing
//! instruction or goal names it, and the desk panel and app windows refuse
//! the remove (only the console answers its question, as with `undo`'s).

use crate::actions::answers::yes_or_no;
use crate::rules::Rules;
use crate::{MAX_REFUSED_RULES, Reply, Shell, refused_rules_file};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

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
        let name = refused_rules_file(n);
        match self.host.refused_rules_read(&name) {
            Ok(Some((_, kept_ns))) => {
                self.refused_asking = Some(n);
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

    /// The person's `y` or `n` to `rules refused remove`'s question. `None`
    /// when none waits, or the line is neither (the question is then left),
    /// or names a variable.
    pub(crate) fn refused_answer(&mut self, input: &str) -> Option<Reply> {
        let n = self.refused_asking.take()?;
        let yes = yes_or_no(input).filter(|_| !self.scope.contains(input))?;
        let name = refused_rules_file(n);
        if !yes {
            return Some(Reply::ok(format!("Kept {name}")));
        }
        Some(match self.host.refused_rules_trash(&name) {
            Ok(()) => {
                self.host.audit(&format!(
                    "rules refused remove {n}: {name} put in the trash"
                ));
                Reply::ok(format!("Put {name} in the trash"))
            }
            Err(why) => Reply::err(format!("{name} was not put in the trash: {why}")),
        })
    }
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
