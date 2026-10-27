//! Package lines (ADR 0011 §9), caught before Rhai: `install notes`, and the
//! other systems' spellings of the same few commands. A leading `sudo` is dropped
//! with a note, and `-y`/`--yes` are ignored: the tier decides, never a flag.

use alloc::string::String;
use alloc::vec::Vec;

/// Longest package name (ADR 0011 §2).
pub const MAX_NAME: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Install(Vec<String>),
    Remove(Vec<String>),
    Upgrade(Option<String>),
    Search(String),
    Info(String),
    List,
    History,
    Rollback(Option<u64>),
}

/// A package line: the command, and whether a `sudo` was dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub command: Command,
    pub sudo: bool,
}

/// The spellings that start each command, longest first so `apt-get install`
/// is not read as `apt` with a stray word.
const INSTALL: &[&[&str]] = &[
    &["apt-get", "install"],
    &["apt", "install"],
    &["dnf", "install"],
    &["brew", "install"],
    &["winget", "install"],
    &["pacman", "-S"],
    &["install"],
];
const REMOVE: &[&[&str]] = &[
    &["apt-get", "remove"],
    &["apt", "remove"],
    &["dnf", "remove"],
    &["brew", "uninstall"],
    &["winget", "uninstall"],
    &["pacman", "-R"],
    &["uninstall"],
    &["remove"],
];
const UPGRADE: &[&[&str]] = &[
    &["apt", "upgrade"],
    &["dnf", "upgrade"],
    &["brew", "upgrade"],
    &["upgrade"],
];

/// Whether `name` is a package name: `[a-z][a-z0-9-]{1,11}`, perhaps with an
/// `@` and a version range after it.
fn is_name(name: &str) -> bool {
    let (base, range) = name.split_once('@').unwrap_or((name, ""));
    let base_ok = (2..=MAX_NAME).contains(&base.len())
        && base.starts_with(|c: char| c.is_ascii_lowercase())
        && base
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let range_ok = !name.contains('@')
        || (!range.is_empty()
            && range.bytes().all(|b| {
                b.is_ascii_digit() || matches!(b, b'.' | b'^' | b'~' | b'=' | b'<' | b'>')
            }));
    base_ok && range_ok
}

fn starts(words: &[&str], spellings: &[&[&str]]) -> Option<usize> {
    spellings
        .iter()
        .find(|s| words.starts_with(s))
        .map(|s| s.len())
}

/// The package names after a command, or why they are not names.
fn names(rest: &[&str]) -> Result<Vec<String>, String> {
    if rest.is_empty() {
        return Err(String::from("name a package, such as `install notes`"));
    }
    rest.iter()
        .map(|word| match is_name(word) {
            true => Ok(String::from(*word)),
            false => Err(alloc::format!("{word} is not a package name")),
        })
        .collect()
}

/// Reads `text` as a package line. `None` when it is not one, so the line goes
/// on to Rhai or the AI; `Some(Err)` when it starts like one but is not right.
pub fn parse(text: &str) -> Option<Result<Line, String>> {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    let sudo = words.first() == Some(&"sudo");
    if sudo {
        words.remove(0);
    }
    words.retain(|w| !matches!(*w, "-y" | "--yes"));
    let command = if let Some(n) = starts(&words, INSTALL) {
        names(&words[n..]).map(Command::Install)
    } else if let Some(n) = starts(&words, REMOVE) {
        names(&words[n..]).map(Command::Remove)
    } else if let Some(n) = starts(&words, UPGRADE) {
        match &words[n..] {
            [] => Ok(Command::Upgrade(None)),
            [name] if is_name(name) => Ok(Command::Upgrade(Some(String::from(*name)))),
            _ => Err(String::from("upgrade takes one package, or none for all")),
        }
    } else {
        let query = |rest: &[&str]| rest.join(" ");
        match words.as_slice() {
            ["pkg", "search", rest @ ..] | ["apt", "search", rest @ ..] if !rest.is_empty() => {
                Ok(Command::Search(query(rest)))
            }
            ["pkg", "info", name] | ["apt", "show", name] => match is_name(name) {
                true => Ok(Command::Info(String::from(*name))),
                false => Err(alloc::format!("{name} is not a package name")),
            },
            ["pkg", "list"] | ["apt", "list", "--installed"] => Ok(Command::List),
            ["pkg", "history"] => Ok(Command::History),
            ["pkg", "rollback"] => Ok(Command::Rollback(None)),
            ["pkg", "rollback", n] => match n.parse::<u64>() {
                Ok(n) if n > 0 => Ok(Command::Rollback(Some(n))),
                _ => Err(String::from("rollback takes a generation number")),
            },
            ["pkg", ..] => Err(String::from(
                "pkg search, info, list, history or rollback [n]",
            )),
            _ => return None,
        }
    };
    Some(command.map(|command| Line { command, sudo }))
}

/// The first word of a package line, which must not be a variable in scope for
/// the line to be one (ADR 0011 §9).
pub fn first_word(text: &str) -> Option<&str> {
    let mut words = text.split_whitespace();
    match words.next()? {
        "sudo" => words.next(),
        word => Some(word),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn install(names: &[&str]) -> Command {
        Command::Install(names.iter().map(|n| String::from(*n)).collect())
    }

    #[test]
    fn every_spelling_of_install_and_remove_reads_alike() {
        for text in [
            "install notes",
            "apt install notes",
            "apt-get install notes",
            "dnf install notes",
            "brew install notes",
            "winget install notes",
            "pacman -S notes",
            "apt install -y notes",
            "install notes --yes",
        ] {
            assert_eq!(
                parse(text),
                Some(Ok(Line {
                    command: install(&["notes"]),
                    sudo: false
                })),
                "{text}"
            );
        }
        for text in [
            "remove notes",
            "uninstall notes",
            "apt remove notes",
            "brew uninstall notes",
            "pacman -R notes",
        ] {
            assert_eq!(
                parse(text).unwrap().unwrap().command,
                Command::Remove(vec![String::from("notes")]),
                "{text}"
            );
        }
    }

    #[test]
    fn sudo_is_dropped_and_said_so() {
        let line = parse("sudo apt install notes spell-en").unwrap().unwrap();
        assert!(line.sudo);
        assert_eq!(line.command, install(&["notes", "spell-en"]));
        assert_eq!(first_word("sudo apt install x"), Some("apt"));
    }

    #[test]
    fn the_other_commands_and_what_is_not_a_package_line() {
        assert_eq!(
            parse("upgrade").unwrap().unwrap().command,
            Command::Upgrade(None)
        );
        assert_eq!(
            parse("pkg search word list").unwrap().unwrap().command,
            Command::Search(String::from("word list"))
        );
        assert_eq!(
            parse("apt show notes").unwrap().unwrap().command,
            Command::Info(String::from("notes"))
        );
        assert_eq!(
            parse("apt list --installed").unwrap().unwrap().command,
            Command::List
        );
        assert_eq!(
            parse("pkg rollback 3").unwrap().unwrap().command,
            Command::Rollback(Some(3))
        );
        assert_eq!(
            parse("install notes@^2.0").unwrap().unwrap().command,
            install(&["notes@^2.0"])
        );
        // Bare search and list stay free for files; other words go on to Rhai.
        for text in [
            "search notes",
            "list",
            "let install = 1",
            "installing is fun",
            "apt",
            "",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
        for text in [
            "install",
            "install Notes",
            "install n",
            "install a-name-too-long",
            "install notes@x",
            "install ../x",
            "pkg rollback zero",
            "pkg frobnicate",
            "upgrade a b",
        ] {
            assert!(matches!(parse(text), Some(Err(_))), "{text}");
        }
    }
}
