//! What the system answers a read with, and what the pages make of it. An answer
//! comes from outside the app, so each is checked: text is cut to a line and
//! stripped of control characters, a list is cut to a count, and anything that
//! does not parse is refused with a reason and leaves the page as it was.
//!
//! | Read | Answer |
//! |---|---|
//! | `rules` | one line a rule: `<n> <words>` or `S<n> <words>` |
//! | `grants` | one line an effect row: `<app>`, `<row>` and `allowed` or `withheld`, tab apart |
//! | `about` | `<key>: <value>` lines for `version`, `commit`, `licence`, `source` and `notices` |
//! | `profiles` | one name a line, the one in use marked `* ` |
//! | `apps` | the names of the apps that can nap, one a line |
//! | `file appearance`, `file apps`, `file updates` | the settings file's bytes |

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use comp_settings::access;
use comp_settings::files::{Appearance, Apps, Updates};
use comp_settings::{RuleLine, RuleRef, Settings};

/// Most lines kept of a list.
pub const MAX_LINES: usize = 64;
/// Longest line kept, in characters.
pub const MAX_LINE: usize = 200;
/// Longest name kept, in bytes.
pub const MAX_NAME: usize = 64;
/// Largest answer read (the host gives no more: `wasmhost::settings::MAX_REPLY`).
pub const MAX_ANSWER: usize = 64 << 10;

/// The reads the window makes when the app starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    Rules,
    Grants,
    About,
    Profiles,
    Apps,
    FileAppearance,
    FileApps,
    FileUpdates,
}

impl Read {
    /// All of them: no more than a call may queue (`wasmhost::settings::MAX_QUEUED`).
    pub const ALL: [Read; 8] = [
        Read::Rules,
        Read::Grants,
        Read::About,
        Read::Profiles,
        Read::Apps,
        Read::FileAppearance,
        Read::FileApps,
        Read::FileUpdates,
    ];

    /// The request that asks for it.
    pub const fn request(self) -> &'static str {
        match self {
            Read::Rules => "rules",
            Read::Grants => "grants",
            Read::About => "about",
            Read::Profiles => "profiles",
            Read::Apps => "apps",
            Read::FileAppearance => "file appearance",
            Read::FileApps => "file apps",
            Read::FileUpdates => "file updates",
        }
    }

    /// Puts the answer on the pages, or says why it cannot.
    pub fn apply(self, settings: &mut Settings, answer: &[u8]) -> Result<(), String> {
        if answer.len() > MAX_ANSWER {
            return Err(String::from("the answer is too large"));
        }
        match self {
            Read::FileAppearance => settings.appearance = Appearance::decode(answer),
            Read::FileApps => settings.apps = Apps::decode(answer),
            Read::FileUpdates => settings.updates = Updates::decode(answer),
            _ => {
                let text = core::str::from_utf8(answer)
                    .map_err(|_| String::from("the answer is not text"))?;
                match self {
                    Read::Rules => settings.rules = rules(text),
                    Read::Grants => {
                        settings.access = access::parse(text)
                            .ok_or_else(|| String::from("the grants are not readable"))?;
                    }
                    Read::About => settings.about = about(text),
                    Read::Profiles => {
                        let (names, current) = profiles(text);
                        settings.profiles = names;
                        settings.profile = current;
                    }
                    Read::Apps => settings.nappable = names(text),
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// Characters that are not drawn as themselves: controls, the zero-width and
/// direction marks, the line and paragraph separators, and the byte-order mark.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}'
        )
}

/// `text` as one line: hidden characters become spaces and it is cut.
fn line(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if hidden(c) { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A name: non-empty, within [`MAX_NAME`] bytes and free of hidden characters.
fn name(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && text.len() <= MAX_NAME && !text.chars().any(hidden))
        .then(|| text.to_string())
}

fn names(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for found in text.lines().filter_map(name) {
        if out.len() < MAX_LINES && !out.contains(&found) {
            out.push(found);
        }
    }
    out
}

fn rules(text: &str) -> Vec<RuleLine> {
    let mut out: Vec<RuleLine> = Vec::new();
    for l in text.lines() {
        let (word, words) = l.split_once(' ').unwrap_or((l, ""));
        // The shell numbers its rules `#2`; the number is what counts.
        let Some(id) = RuleRef::parse(word.strip_prefix('#').unwrap_or(word)) else {
            continue;
        };
        if out.len() < MAX_LINES && !out.iter().any(|r| r.id == id) {
            out.push(RuleLine {
                id,
                text: line(words, MAX_LINE),
            });
        }
    }
    out
}

fn about(text: &str) -> comp_settings::About {
    let mut about = comp_settings::About::default();
    for l in text.lines() {
        let Some((key, value)) = l.split_once(": ") else {
            continue;
        };
        let value = line(value, MAX_LINE);
        match key {
            "version" => about.version = value,
            "commit" => about.commit = value,
            "licence" => about.licence = value,
            "source" => about.source = value,
            "notices" => about.notices = value,
            _ => {}
        }
    }
    about
}

/// The names, and the one marked `* `.
fn profiles(text: &str) -> (Vec<String>, Option<String>) {
    let mut current = None;
    let mut all = Vec::new();
    for l in text.lines() {
        let (marked, rest) = match l.strip_prefix("* ") {
            Some(rest) => (true, rest),
            None => (false, l),
        };
        let Some(found) = name(rest) else { continue };
        if all.len() < MAX_LINES && !all.contains(&found) {
            if marked && current.is_none() {
                current = Some(found.clone());
            }
            all.push(found);
        }
    }
    (all, current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmhost::settings::{Kind, MAX_QUEUED, classify};

    fn read(read: Read, text: &str) -> Settings {
        let mut s = Settings::default();
        read.apply(&mut s, text.as_bytes()).unwrap();
        s
    }

    #[test]
    fn the_start_reads_fit_one_call_and_are_all_reads() {
        assert!(Read::ALL.len() <= MAX_QUEUED);
        for r in Read::ALL {
            assert_eq!(classify(r.request()), Ok(Kind::Read), "{}", r.request());
        }
    }

    #[test]
    fn rules_are_numbered_lines_and_standing_instructions_carry_an_s() {
        let s = read(
            Read::Rules,
            "1 allow stop of cache\nS2 on minimize(notes) -> app_suspend(notes)\nbad line\n1 again\n\u{1b}[31m3 x",
        );
        assert_eq!(s.rules.len(), 2);
        assert_eq!(s.rules[0].id, RuleRef::Rule(1));
        assert_eq!(s.rules[0].text, "allow stop of cache");
        assert_eq!(s.rules[1].id, RuleRef::Instruction(2));
    }

    #[test]
    fn the_shells_numbered_lines_read_too() {
        let s = read(
            Read::Rules,
            "#1 allow stop of cache\n#S2 on idle -> x\n#x nope",
        );
        assert_eq!(s.rules.len(), 2);
        assert_eq!(s.rules[0].id, RuleRef::Rule(1));
        assert_eq!(s.rules[1].id, RuleRef::Instruction(2));
    }

    #[test]
    fn characters_that_are_not_drawn_as_themselves_are_taken_out() {
        let s = read(
            Read::Rules,
            "1 allow\u{202e}evil\u{200b}\u{2028}tail\n2 ok\u{feff}",
        );
        assert_eq!(s.rules[0].text, "allow evil  tail");
        assert_eq!(s.rules[1].text, "ok");
        let s = read(Read::Apps, "no\u{200b}tes\nfiles\n");
        assert_eq!(s.nappable, ["files"]);
        let s = read(Read::Profiles, "qui\u{202e}et\nfocus");
        assert_eq!(s.profiles, ["focus"]);
    }

    #[test]
    fn an_answer_over_the_host_s_limit_is_refused() {
        let mut s = Settings::default();
        let big = alloc::vec![b'a'; MAX_ANSWER + 1];
        assert!(Read::Apps.apply(&mut s, &big).is_err());
        assert!(Read::FileAppearance.apply(&mut s, &big).is_err());
        assert!(s.nappable.is_empty());
        let edge = alloc::vec![b'\n'; MAX_ANSWER];
        assert!(Read::Apps.apply(&mut s, &edge).is_ok());
        assert_eq!(MAX_ANSWER, wasmhost::settings::MAX_REPLY);
    }

    #[test]
    fn about_takes_its_known_keys_and_cuts_the_rest() {
        let long = "x".repeat(500);
        let s = read(
            Read::About,
            &format!(
                "version: 0.1.0-dev\ncommit: abc123\nlicence: MPL-2.0\nsource: github.com/x/y\nnotices: {long}\nother: 1\nno colon"
            ),
        );
        assert_eq!(s.about.version, "0.1.0-dev");
        assert_eq!(s.about.licence, "MPL-2.0");
        assert_eq!(s.about.notices.chars().count(), MAX_LINE);
    }

    #[test]
    fn profiles_mark_the_one_in_use() {
        let s = read(
            Read::Profiles,
            "balanced\n* quiet\nfocus\n* second\nquiet\n",
        );
        assert_eq!(s.profiles, ["balanced", "quiet", "focus", "second"]);
        assert_eq!(s.profile.as_deref(), Some("quiet"));
        let s = read(Read::Profiles, "a\nb");
        assert_eq!(s.profile, None);
    }

    #[test]
    fn apps_are_names_without_repeats_or_control_characters() {
        let s = read(Read::Apps, "notes\nfiles\nnotes\nbad\u{7}name\n\n");
        assert_eq!(s.nappable, ["notes", "files"]);
    }

    #[test]
    fn lists_are_cut_to_a_count() {
        let many: String = (0..200).map(|i| format!("app{i}\n")).collect();
        assert_eq!(read(Read::Apps, &many).nappable.len(), MAX_LINES);
        let rules: String = (1..200).map(|i| format!("{i} rule\n")).collect();
        assert_eq!(read(Read::Rules, &rules).rules.len(), MAX_LINES);
    }

    #[test]
    fn the_grants_are_the_shells_effect_rows() {
        let mut s = Settings::default();
        assert!(Read::Grants.apply(&mut s, b"not a row").is_err());
        assert!(Read::Grants.apply(&mut s, &[0xff, 0xfe]).is_err());
        assert!(s.access.is_empty());
        Read::Grants.apply(&mut s, b"").unwrap();
        assert!(s.access.is_empty());
        let rows = "notes\tnotes_add\tallowed\nnotes\tfs_write\twithheld\n";
        Read::Grants.apply(&mut s, rows.as_bytes()).unwrap();
        assert_eq!(s.access.len(), 1);
        assert_eq!(s.access[0].access.len(), 2);
        // A later answer with nothing readable leaves the page as it was.
        assert!(Read::Grants.apply(&mut s, b"junk").is_err());
        assert_eq!(s.access.len(), 1);
    }

    /// What the app takes from the shell is what it can send back.
    #[test]
    fn every_row_listed_is_a_row_the_host_takes_in_a_request() {
        let rows = "a-b\tfs_list\tallowed\nc2\tx9\twithheld\n";
        let mut s = Settings::default();
        Read::Grants.apply(&mut s, rows.as_bytes()).unwrap();
        for app in &s.access {
            let read = alloc::format!("grants {}", app.app);
            assert_eq!(classify(&read), Ok(Kind::Read), "{read}");
            for x in &app.access {
                for verb in ["grants-revoke", "grants-restore"] {
                    let request = alloc::format!("{verb} {} {}", app.app, x.row);
                    assert_eq!(classify(&request), Ok(Kind::Change), "{request}");
                }
            }
        }
    }

    #[test]
    fn a_settings_file_that_cannot_be_read_is_the_default() {
        let mut s = Settings::default();
        let before = s.clone();
        Read::FileAppearance.apply(&mut s, b"junk").unwrap();
        Read::FileApps.apply(&mut s, &[0xff; 32]).unwrap();
        Read::FileUpdates.apply(&mut s, b"").unwrap();
        assert_eq!(s, before);
    }

    #[test]
    fn a_settings_file_is_read_with_its_own_decoder() {
        let mut s = Settings::default();
        let apps = Apps {
            keep_running: alloc::vec![String::from("notes")],
        };
        Read::FileApps.apply(&mut s, &apps.encode()).unwrap();
        assert!(s.apps.keeps("notes"));
        let updates = Updates {
            restart_window: Some((120, 300)),
        };
        Read::FileUpdates.apply(&mut s, &updates.encode()).unwrap();
        assert_eq!(s.updates.restart_window, Some((120, 300)));
    }
}
