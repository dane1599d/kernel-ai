//! What to write instead, after wcc's commonest messages. wcc names what is
//! wrong at the token where it stopped, which is not always what to change: a
//! missing `;` is reported at the next statement, and a call to `rand` as
//! "cannot call except function". For the slips the app gate saw the model
//! repeat through all its repairs, and the other common ones, a `hint:` line
//! after the message says what to write, for the person at `c>` and the
//! model's repairs alike. wcc itself is unmodified (third_party/xcc).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a hint's line starts with.
pub const HINT: &str = "hint: ";

/// The C library a source compiles against, which decides what a hint
/// offers in place of a function it does not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Library {
    /// A `c>` session: `kai.h`'s `Print`, and kairt.
    Session,
    /// A `cc build` component or app: `<string.h>` and `<stdlib.h>`'s
    /// allocation.
    Component,
    /// A window app: those, and `<kaiapp.h>`'s `Format` and `ui_` calls.
    Window,
}

/// wcc's message after an undeclared name that is called.
const CANNOT_CALL: &str = "cannot call except function";
/// How wcc's link names a symbol nothing defines.
const UNRESOLVED: &str = "Unresolved: ";
/// Functions no library here has, with the same hint.
const RANDOM: &[&str] = &["rand", "srand", "random", "srandom"];
const PRINTING: &[&str] = &[
    "printf", "puts", "putchar", "sprintf", "snprintf", "fprintf",
];

/// wcc's messages that always get the same hint.
const FIXED: &[(&str, &str)] = &[
    (
        "Character not closed",
        r"a character literal is one character in single quotes, closed on the same line: write '\n' for a newline and '\'' for a quote. More than one character is a string, in double quotes.",
    ),
    (
        "Empty character",
        r"'' holds no character: write ' ' for a space or '\0' for the zero byte.",
    ),
    (
        "String not closed",
        r#"a string must close on the line it opens: write \n for a newline in it and \" for a quote. To go on to the next line, close it and open another there: "ab" "cd" is "abcd"."#,
    ),
    (
        "Illegal escape",
        r#"the escapes are \n, \t, \r, \0, \\, \', \" and \x with hex digits."#,
    ),
    (
        "`;' expected",
        "the statement before this point has no ';' at its end, often on the line above: add it there.",
    ),
];

/// One of wcc's messages and the lines that show where.
struct Block<'a> {
    /// The file and line it names; none for the link's.
    at: Option<(&'a str, u32)>,
    message: &'a str,
    lines: Vec<&'a str>,
}

/// `messages`, wcc's, with a hint after each message that has one, each
/// hint once.
pub fn add(messages: &str, library: Library) -> String {
    let blocks = blocks(messages);
    let mut out = String::with_capacity(messages.len());
    let mut given: Vec<String> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        for line in &block.lines {
            out.push_str(line);
            out.push('\n');
        }
        let Some(hint) = hint(&blocks, i, library) else {
            continue;
        };
        if !given.contains(&hint) {
            out.push_str(HINT);
            out.push_str(&hint);
            out.push('\n');
            given.push(hint);
        }
    }
    out
}

/// wcc's `/w/<file>(<line>): <message>`.
fn located(line: &str) -> Option<((&str, u32), &str)> {
    let (file, rest) = line.strip_prefix("/w/")?.split_once('(')?;
    let (n, message) = rest.split_once("): ")?;
    Some(((file, n.parse().ok()?), message))
}

/// `messages` cut into one block a message, each with the lines after it.
fn blocks(messages: &str) -> Vec<Block<'_>> {
    let mut blocks: Vec<Block<'_>> = Vec::new();
    for line in messages.lines() {
        let head = match located(line) {
            Some((at, message)) => Some((Some(at), message)),
            None if line.starts_with(UNRESOLVED) => Some((None, line)),
            None => None,
        };
        match (head, blocks.last_mut()) {
            (Some((at, message)), _) => blocks.push(Block {
                at,
                message,
                lines: alloc::vec![line],
            }),
            (None, Some(block)) => block.lines.push(line),
            (None, None) => blocks.push(Block {
                at: None,
                message: "",
                lines: alloc::vec![line],
            }),
        }
    }
    blocks
}

/// The hint after block `i`. wcc follows an undeclared name that is called
/// with [`CANNOT_CALL`] at the same place, and the hint comes after both.
fn hint(blocks: &[Block<'_>], i: usize, library: Library) -> Option<String> {
    let block = &blocks[i];
    let called = |name: &Block<'_>, call: &Block<'_>| {
        call.message == CANNOT_CALL && call.at == name.at && undeclared(name.message).is_some()
    };
    if let Some(symbol) = block.message.strip_prefix(UNRESOLVED) {
        let name = symbol.rsplit('.').next().unwrap_or(symbol);
        return Some(missing(name, library).unwrap_or_else(|| {
            format!("`{name}' is declared, but no file defines it and no library here has it: define it.")
        }));
    }
    if let Some(previous) = i.checked_sub(1).map(|p| &blocks[p])
        && called(previous, block)
    {
        let name = undeclared(previous.message)?;
        return Some(missing(name, library).unwrap_or_else(|| {
            format!("`{name}' is called but not declared: define it above its first call, or include the header that declares it.")
        }));
    }
    if let Some(name) = undeclared(block.message) {
        let is_call = blocks.get(i + 1).is_some_and(|next| called(block, next));
        return match (is_call, name) {
            (false, "true" | "false") => {
                Some("true and false come from <stdbool.h>: include it, or write 1 and 0.".into())
            }
            _ => None,
        };
    }
    FIXED
        .iter()
        .find(|(message, _)| *message == block.message)
        .map(|(_, hint)| hint.to_string())
}

/// The name in wcc's "`<name>' undeclared".
fn undeclared(message: &str) -> Option<&str> {
    message.strip_prefix('`')?.strip_suffix("' undeclared")
}

/// What to use in place of `name`, a function no library here has.
fn missing(name: &str, library: Library) -> Option<String> {
    if RANDOM.contains(&name) {
        return Some(format!(
            "there is no {name}() here: keep an unsigned long long seed (an app keeps it in struct state) and step it, seed = seed * 6364136223846793005 + 1442695040888963407; then (seed >> 33) % n is below n."
        ));
    }
    if !PRINTING.contains(&name) {
        return None;
    }
    match library {
        Library::Session => Some(format!(
            "there is no {name}() here: Print(fmt, ...) prints as printf does."
        )),
        Library::Window => Some(format!(
            "there is no {name}() here: Format(out, cap, fmt, ...) writes into a buffer (%lld, %s, %c, %%), and ui_label shows it."
        )),
        Library::Component => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAR: &str = "/w/main.c(8): Character not closed\n    char c = '\n\n";
    const RAND: &str = "/w/main.c(8): `rand' undeclared\n    x = rand() % 6;\n        ^~~~\n\
        /w/main.c(8): cannot call except function\n    x = rand() % 6;\n        ^~~~\n";

    #[test]
    fn a_character_split_by_a_newline_is_told_to_write_backslash_n() {
        let got = add(CHAR, Library::Window);
        assert_eq!(got, format!("{CHAR}{HINT}{}\n", FIXED[0].1));
        assert!(got.contains(r"write '\n' for a newline"), "{got}");
    }

    #[test]
    fn each_fixed_message_gets_its_hint_after_the_lines_that_show_it() {
        for (message, hint) in FIXED {
            let text = format!("/w/entry.c(3): {message}\n  x\n  ^\n");
            assert_eq!(
                add(&text, Library::Session),
                format!("{text}{HINT}{hint}\n")
            );
        }
    }

    #[test]
    fn a_call_to_rand_gets_one_hint_with_a_generator() {
        let got = add(RAND, Library::Window);
        assert_eq!(got.matches(HINT).count(), 1, "{got}");
        assert!(got.ends_with("(seed >> 33) % n is below n.\n"), "{got}");
        assert!(got.contains("there is no rand() here"), "{got}");
    }

    #[test]
    fn printf_is_told_what_each_library_prints_with() {
        let text = RAND.replace("rand", "printf");
        assert!(add(&text, Library::Session).contains("Print(fmt, ...) prints"));
        assert!(add(&text, Library::Window).contains("Format(out, cap, fmt, ...)"));
        let component = add(&text, Library::Component);
        assert!(
            component.contains("`printf' is called but not declared"),
            "{component}"
        );
    }

    #[test]
    fn another_undeclared_call_is_told_to_declare_it() {
        let got = add(&RAND.replace("rand", "tick"), Library::Window);
        assert!(got.ends_with(
            "hint: `tick' is called but not declared: define it above its first call, or include the header that declares it.\n"
        ), "{got}");
    }

    #[test]
    fn an_undeclared_variable_gets_no_hint_but_true_names_stdbool() {
        let nope = "/w/main.c(8): `nope' undeclared\n    f(nope);\n      ^~~~\n";
        assert_eq!(add(nope, Library::Window), nope);
        let on = nope.replace("nope", "true");
        assert!(
            add(&on, Library::Window).ends_with("<stdbool.h>: include it, or write 1 and 0.\n")
        );
    }

    #[test]
    fn a_name_the_link_cannot_find_is_named_with_what_to_do() {
        let got = add("Unresolved: env.rand\n", Library::Window);
        assert!(
            got.starts_with("Unresolved: env.rand\nhint: there is no rand() here"),
            "{got}"
        );
        let got = add("Unresolved: env.tick\n", Library::Component);
        assert!(got.ends_with("hint: `tick' is declared, but no file defines it and no library here has it: define it.\n"), "{got}");
    }

    #[test]
    fn a_hint_is_given_once_and_unknown_messages_pass_unchanged() {
        let twice = format!("{CHAR}{CHAR}");
        assert_eq!(add(&twice, Library::Window).matches(HINT).count(), 1);
        let other = "/w/main.c(2): illegal type combination\n  long char x;\n";
        assert_eq!(add(other, Library::Window), other);
        assert_eq!(add("", Library::Window), "");
        assert_eq!(add("noise first\n", Library::Session), "noise first\n");
    }
}
