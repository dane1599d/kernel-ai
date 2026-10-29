//! The OS rows the app asks for, as effects: a registry name and its arguments
//! as a WAVE record (ADR 0003 section 4). The app queues these; the agent core
//! applies each row's tier, logs it and records its undo, so nothing here can
//! change a file by itself. The limits are `livecore::txn`'s: an effect over them
//! traps the whole call, so a call that would go over is refused first.

use crate::names::path_ok;
use alloc::format;
use alloc::string::String;

/// Longest argument text of an effect call (`livecore::txn::MAX_EFFECT_ARGS`).
pub const MAX_ARGS: usize = 512;

/// One effect: an OS row by name and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub name: &'static str,
    pub args: String,
}

/// A WAVE string: quoted, with `\` and `"` and control characters escaped as
/// `appabi::wave` prints them.
pub fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn call(name: &'static str, args: String) -> Option<Call> {
    (args.len() <= MAX_ARGS).then_some(Call { name, args })
}

fn one(name: &'static str, path: &str) -> Option<Call> {
    if !path_ok(path) {
        return None;
    }
    call(name, format!("{{path: {}}}", quote(path)))
}

/// `fs_list`: a folder's names.
pub fn list(path: &str) -> Option<Call> {
    one("fs_list", path)
}

/// `fs_read`: a file's text.
pub fn read(path: &str) -> Option<Call> {
    one("fs_read", path)
}

/// `fs_trash`: to the trash; `undo` brings it back.
pub fn trash(path: &str) -> Option<Call> {
    one("fs_trash", path)
}

/// `fs_restore`: the newest trashed item that was at `path`, back there.
pub fn restore(path: &str) -> Option<Call> {
    one("fs_restore", path)
}

/// `fs_move`: moves or renames.
pub fn mv(from: &str, to: &str) -> Option<Call> {
    if !path_ok(from) || !path_ok(to) {
        return None;
    }
    call(
        "fs_move",
        format!("{{from: {}, to: {}}}", quote(from), quote(to)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use appabi::{Ty, Val, wave};

    #[test]
    fn quote_prints_what_wave_prints_and_parses_back() {
        let samples = [
            "",
            "plain.txt",
            "say \"hi\"",
            "back\\slash",
            "tab\there",
            "line\nbreak",
            "bell\u{7}",
            "\u{85}next",
            "emoji \u{1F600}",
            "\\u{41}",
            "\"}, to: \"x",
        ];
        for s in samples {
            let q = quote(s);
            let printed = wave::print(&Val::String(s.into()), &Ty::String).unwrap();
            assert_eq!(q, printed, "{s:?}");
            assert_eq!(wave::parse(&q, &Ty::String).unwrap(), Val::String(s.into()));
        }
    }

    #[test]
    fn calls_are_records_the_agent_core_reads() {
        let ty = Ty::Record(vec![("path".into(), Ty::String)]);
        let c = trash("docs/a \"b\".txt").unwrap();
        assert_eq!(c.name, "fs_trash");
        let want = Val::Record(vec![Val::String("docs/a \"b\".txt".into())]);
        assert_eq!(wave::parse(&c.args, &ty).unwrap(), want);
        let ty = Ty::Record(vec![("from".into(), Ty::String), ("to".into(), Ty::String)]);
        let c = mv("a", "b/c").unwrap();
        assert_eq!(c.name, "fs_move");
        let want = Val::Record(vec![Val::String("a".into()), Val::String("b/c".into())]);
        assert_eq!(wave::parse(&c.args, &ty).unwrap(), want);
    }

    #[test]
    fn a_path_a_row_would_refuse_makes_no_call() {
        assert!(list("../etc").is_none());
        assert!(read("/abs").is_none());
        assert!(mv("a", "../b").is_none());
        assert!(list("").is_some());
    }

    #[test]
    fn a_call_over_the_effect_limit_makes_no_call() {
        // Quotes double in WAVE, so two long paths of them cannot both fit in
        // 512 bytes, though each is a fine path.
        let doubled = "\"".repeat(150);
        assert!(path_ok(&doubled));
        assert!(trash(&doubled).is_some());
        assert!(mv(&doubled, &doubled).is_none());
        // Two plain paths fit up to about 240 bytes each; 250 each do not.
        let plain = "a".repeat(240);
        assert!(mv(&plain, &plain).is_some());
        let plain = "a".repeat(250);
        assert!(mv(&plain, &plain).is_none());
    }
}
