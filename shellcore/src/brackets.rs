//! Whether a typed entry is unfinished: a brace, bracket or parenthesis still open,
//! or a block comment or backtick string not yet closed. The shell then replies
//! `more` and waits for the next line (ADR 0012).
//!
//! Brackets inside strings, characters and comments do not count. A closing bracket
//! with nothing open makes the entry finished, so Rhai reports the error.

/// True if `code` needs another line before it can run.
pub fn open(code: &str) -> bool {
    let mut chars = code.chars().peekable();
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => match depth.checked_sub(1) {
                Some(less) => depth = less,
                None => return false,
            },
            '"' | '\'' => skip_quoted(&mut chars, c),
            '`' => {
                if !skip_backtick(&mut chars) {
                    return true;
                }
            }
            '/' if chars.peek() == Some(&'/') => while chars.next_if(|&c| c != '\n').is_some() {},
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                if !skip_block_comment(&mut chars) {
                    return true;
                }
            }
            _ => {}
        }
    }
    depth > 0
}

type Chars<'a> = core::iter::Peekable<core::str::Chars<'a>>;

/// Skips a `"` string or `'` character to its closing quote. A newline ends it too:
/// neither may hold one, so Rhai reports it rather than waiting for more.
fn skip_quoted(chars: &mut Chars, quote: char) {
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '\n' => return,
            c if c == quote => return,
            _ => {}
        }
    }
}

/// Skips a backtick string, which may span lines; false if it never closes.
fn skip_backtick(chars: &mut Chars) -> bool {
    chars.any(|c| c == '`')
}

/// Skips a block comment, which nests in Rhai; false if it never closes.
fn skip_block_comment(chars: &mut Chars) -> bool {
    let mut depth = 1usize;
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.next_if_eq(&'*').is_some() => depth += 1,
            '*' if chars.next_if_eq(&'/').is_some() => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}
