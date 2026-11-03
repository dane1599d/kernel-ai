//! Cheap fixes before a model's Kai C program is compiled (ADR 0012, spike
//! S12-2's re-run). [`prepare`] adds a standard include the program needs but
//! left out, and finds mistakes wcc lets through or reports badly: a variable
//! named after a kai.h call (`I64 cpus;`, which wcc reports as a type conflict
//! at best), and a `Print` whose conversions do not fit its arguments (`%d` or
//! `%f` for an I64, or a count that does not match). A problem goes back to the
//! model as a compile error would, with the entry's line number.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// `<string.h>`'s functions: a program that calls one gets the include.
const STRING_H: [&str; 14] = [
    "strlen", "strcmp", "strncmp", "strcpy", "strncpy", "strcat", "strncat", "strchr", "strrchr",
    "strstr", "memcpy", "memset", "memcmp", "memmove",
];

/// Type words that start a declaration.
const TYPES: [&str; 9] = [
    "I64", "int", "long", "char", "short", "unsigned", "signed", "double", "float",
];

/// A program after the fixes, and what is still wrong with it.
#[derive(Debug, PartialEq, Eq)]
pub struct Prepared {
    pub source: String,
    /// One line each, `line N: ...`; empty when the program may be compiled.
    pub problems: Vec<String>,
}

/// Fixes and checks `source`, where `calls` are the kai.h functions it may call.
pub fn prepare(source: &str, calls: &[&str]) -> Prepared {
    let tokens = lex(source);
    let needs_string_h = tokens
        .iter()
        .any(|t| matches!(&t.kind, Kind::Ident(name) if STRING_H.contains(&name.as_str())));
    let has_string_h = source
        .lines()
        .any(|line| line.trim_start().starts_with("#include") && line.contains("<string.h>"));
    let (source, shift) = match needs_string_h && !has_string_h {
        true => (format!("#include <string.h>\n{source}"), 1),
        false => (source.to_string(), 0),
    };
    let mut problems = shadowed(&tokens, calls);
    problems.extend(formats(&tokens));
    problems.sort_by_key(|(line, _)| *line);
    Prepared {
        source,
        problems: problems
            .into_iter()
            .map(|(line, text)| format!("line {}: {text}", line + shift))
            .collect(),
    }
}

/// Declarations that name a variable after a function.
fn shadowed(tokens: &[Token], calls: &[&str]) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let Kind::Ident(word) = &token.kind else {
            continue;
        };
        let after_struct = i >= 1 && tokens[i - 1].is_ident("struct");
        if !TYPES.contains(&word.as_str()) && !after_struct {
            continue;
        }
        let mut j = i + 1;
        while tokens.get(j).is_some_and(|t| t.is_punct('*')) {
            j += 1;
        }
        let Some(Token {
            kind: Kind::Ident(name),
            line,
        }) = tokens.get(j)
        else {
            continue;
        };
        let function =
            calls.contains(&name.as_str()) || STRING_H.contains(&name.as_str()) || name == "Print";
        if function && !tokens.get(j + 1).is_some_and(|t| t.is_punct('(')) {
            found.push((
                *line,
                format!(
                    "`{name}` is a function, so a variable may not take its name; call it {name}_n or another name"
                ),
            ));
        }
    }
    found
}

/// `Print` calls whose conversions do not fit the I64 rule or the arguments.
fn formats(tokens: &[Token]) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !token.is_ident("Print") || !tokens.get(i + 1).is_some_and(|t| t.is_punct('(')) {
            continue;
        }
        let mut j = i + 2;
        let mut format_text = String::new();
        while let Some(Token {
            kind: Kind::Str(text),
            ..
        }) = tokens.get(j)
        {
            format_text.push_str(text);
            j += 1;
        }
        if j == i + 2 {
            continue;
        }
        let Some(arguments) = count_arguments(&tokens[j..]) else {
            continue;
        };
        let (wanted, wrong) = conversions(&format_text);
        for spec in wrong {
            found.push((
                token.line,
                format!(
                    "Print's %{spec} does not take an I64: use %lld (whole numbers only, no %f)"
                ),
            ));
        }
        if wanted != arguments {
            found.push((
                token.line,
                format!("Print's format wants {wanted} values but gets {arguments}"),
            ));
        }
    }
    found
}

/// The arguments after a format, `tokens` starting after its last string;
/// `None` when the call does not close.
fn count_arguments(tokens: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    let mut count = 0;
    for token in tokens {
        match &token.kind {
            Kind::Punct('(' | '[' | '{') => depth += 1,
            Kind::Punct(')' | ']' | '}') if depth == 0 => return Some(count),
            Kind::Punct(')' | ']' | '}') => depth -= 1,
            Kind::Punct(',') if depth == 0 => count += 1,
            _ => {}
        }
    }
    None
}

/// How many values `format` reads, and its conversions an I64 does not fit.
fn conversions(format: &str) -> (usize, Vec<String>) {
    let bytes = format.as_bytes();
    let (mut wanted, mut wrong, mut i) = (0, Vec::new(), 0);
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        let start = i + 1;
        i += 1;
        if bytes.get(i) == Some(&b'%') {
            i += 1;
            continue;
        }
        while i < bytes.len() && b"-+ 0#".contains(&bytes[i]) {
            i += 1;
        }
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'*' || bytes[i] == b'.')
        {
            wanted += usize::from(bytes[i] == b'*');
            i += 1;
        }
        let length_start = i;
        while i < bytes.len() && b"hlzjtL".contains(&bytes[i]) {
            i += 1;
        }
        let Some(&conversion) = bytes.get(i) else {
            break;
        };
        i += 1;
        wanted += 1;
        let length = &format[length_start..i - 1];
        let fits = match conversion {
            b'd' | b'i' | b'u' | b'x' | b'X' | b'o' => length == "ll",
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => false,
            _ => true,
        };
        if !fits {
            wrong.push(format[start..i].to_string());
        }
    }
    (wanted, wrong)
}

#[derive(Debug)]
enum Kind {
    Ident(String),
    Str(String),
    Punct(char),
}

#[derive(Debug)]
struct Token {
    kind: Kind,
    /// 1-based, in the source as given.
    line: u32,
}

impl Token {
    fn is_ident(&self, word: &str) -> bool {
        matches!(&self.kind, Kind::Ident(name) if name == word)
    }

    fn is_punct(&self, c: char) -> bool {
        matches!(self.kind, Kind::Punct(p) if p == c)
    }
}

/// Identifiers, string literals (escapes kept as written) and punctuation;
/// comments, numbers, character literals and `#` lines are skipped.
fn lex(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let (mut tokens, mut line, mut i) = (Vec::new(), 1u32, 0);
    let mut line_start = true;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\n' => {
                line += 1;
                line_start = true;
                i += 1;
                continue;
            }
            '#' if line_start => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if next == Some('/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if next == Some('*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    line += u32::from(chars[i] == '\n');
                    i += 1;
                }
                i += 2;
            }
            '"' | '\'' => {
                let mut text = String::new();
                i += 1;
                while i < chars.len() && chars[i] != c && chars[i] != '\n' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        text.push(chars[i]);
                        i += 1;
                    }
                    text.push(chars[i]);
                    i += 1;
                }
                i += 1;
                if c == '"' {
                    tokens.push(Token {
                        kind: Kind::Str(text),
                        line,
                    });
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(Token {
                    kind: Kind::Ident(chars[start..i].iter().collect()),
                    line,
                });
            }
            c if c.is_ascii_digit() => {
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
            }
            c if c.is_whitespace() => i += 1,
            c => {
                tokens.push(Token {
                    kind: Kind::Punct(c),
                    line,
                });
                i += 1;
            }
        }
        line_start = false;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALLS: [&str; 3] = ["cpus", "memory", "processes"];

    #[test]
    fn a_program_calling_strlen_gets_string_h() {
        let out = prepare("char p[8];\nPrint(\"%lld\\n\", (I64)strlen(p));", &CALLS);
        assert!(out.source.starts_with("#include <string.h>\nchar p[8];"));
        assert_eq!(out.problems, Vec::<String>::new());
        let kept = prepare("#include <string.h>\nI64 n = strlen(\"ab\");", &CALLS);
        assert_eq!(kept.source, "#include <string.h>\nI64 n = strlen(\"ab\");");
    }

    #[test]
    fn a_variable_named_after_a_call_is_a_problem() {
        let out = prepare("struct kai_memory m;\nI64 cpus;\ncpus(&cpus);", &CALLS);
        assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
        assert!(out.problems[0].starts_with("line 2: `cpus` is a function"));
        let fine = prepare(
            "I64 n_cpus;\ncpus(&n_cpus);\nstruct kai_memory memory_now;",
            &CALLS,
        );
        assert_eq!(fine.problems, Vec::<String>::new());
    }

    #[test]
    fn print_conversions_must_fit_an_i64_and_the_arguments() {
        let out = prepare(
            "I64 a;\nPrint(\"%dm%ds\\n\", a, a);\nPrint(\"%lld %lld\\n\", a);",
            &CALLS,
        );
        assert_eq!(out.problems.len(), 3, "{:?}", out.problems);
        assert!(out.problems[0].starts_with("line 2: Print's %d"));
        assert!(out.problems[2].starts_with("line 3: Print's format wants 2 values but gets 1"));
        let float = prepare("Print(\"%.3lf\\n\", 1.5);", &CALLS);
        assert!(float.problems[0].contains("%.3lf"), "{:?}", float.problems);
        let fine = prepare(
            "Print(\"%lld%% %s %03lld %c\\n\", f(1, 2), \"a,b\", x[1], 'c');",
            &CALLS,
        );
        assert_eq!(fine.problems, Vec::<String>::new());
    }

    #[test]
    fn an_added_include_moves_the_line_numbers() {
        let out = prepare("I64 cpus;\nI64 n = strlen(\"a\");", &CALLS);
        assert_eq!(out.problems.len(), 1);
        assert!(out.problems[0].starts_with("line 2:"), "{:?}", out.problems);
    }

    #[test]
    fn comments_and_strings_are_not_read_as_code() {
        let out = prepare(
            "// I64 cpus;\n/* I64 memory; */\nPrint(\"I64 cpus;\\n\");",
            &CALLS,
        );
        assert_eq!(out.problems, Vec::<String>::new());
    }
}
