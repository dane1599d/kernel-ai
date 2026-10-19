//! The qwen2 pretokenizer, by hand. It cuts text where this regex (llama.cpp's
//! spelling of Qwen2's) would, trying the alternatives in order at each position:
//!
//! ```text
//! (?:'[sS]|'[tT]|'[rR][eE]|'[vV][eE]|'[mM]|'[lL][lL]|'[dD])
//! |[^\r\n\p{L}\p{N}]?\p{L}+
//! |\p{N}
//! | ?[^\s\p{L}\p{N}]+[\r\n]*
//! |\s*[\r\n]+
//! |\s+(?!\S)
//! |\s+
//! ```
//!
//! \p{N} is `char::is_numeric` and \s is `char::is_whitespace`, both exact. \p{L}
//! is `char::is_alphabetic` less the numeric chars and less the marks and symbols
//! Unicode calls alphabetic ([`crate::marks`]), exact for every char Unicode 16
//! assigns, so Indic, Thai and vocalised Arabic words cut as llama.cpp cuts them.
//! Text is not NFC-normalised first: that matches llama.cpp, while Hugging Face's
//! tokenizer would recompose decomposed accents.

use crate::marks::alphabetic_non_letter;

fn is_letter(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic()
    } else {
        c.is_alphabetic() && !c.is_numeric() && !alphabetic_non_letter(c)
    }
}

fn is_number(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        c.is_numeric()
    }
}

fn is_newline(c: char) -> bool {
    c == '\r' || c == '\n'
}

/// Neither whitespace, a letter nor a number.
fn is_other(c: char) -> bool {
    !c.is_whitespace() && !is_letter(c) && !is_number(c)
}

/// Bytes in the longest prefix of chars that `class` accepts.
fn run(s: &str, class: impl Fn(char) -> bool) -> usize {
    s.find(|c| !class(c)).unwrap_or(s.len())
}

/// The bytes after an apostrophe that make a contraction: s, t, re, ve, m, ll, d.
fn contraction(rest: &str) -> Option<usize> {
    let lower = |i: usize| rest.as_bytes().get(i).map(u8::to_ascii_lowercase);
    match (lower(0)?, lower(1)) {
        (b's' | b't' | b'm' | b'd', _) => Some(1),
        (b'r' | b'v', Some(b'e')) | (b'l', Some(b'l')) => Some(2),
        _ => None,
    }
}

/// The length in bytes of the piece `s` starts with; at least one char unless
/// `s` is empty.
fn piece_len(s: &str) -> usize {
    let Some(c0) = s.chars().next() else {
        return 0;
    };
    let len0 = c0.len_utf8();
    let rest = &s[len0..];
    let c1 = rest.chars().next();
    if c0 == '\''
        && let Some(n) = contraction(rest)
    {
        return len0 + n;
    }
    // [^\r\n\p{L}\p{N}]?\p{L}+
    if is_letter(c0) {
        return run(s, is_letter);
    }
    if !is_newline(c0) && !is_number(c0) && c1.is_some_and(is_letter) {
        return len0 + run(rest, is_letter);
    }
    // \p{N}
    if is_number(c0) {
        return len0;
    }
    // ` ?[^\s\p{L}\p{N}]+[\r\n]*`
    let start = if c0 == ' ' && c1.is_some_and(is_other) {
        Some(len0)
    } else if is_other(c0) {
        Some(0)
    } else {
        None
    };
    if let Some(start) = start {
        let end = start + run(&s[start..], is_other);
        return end + run(&s[end..], is_newline);
    }
    // From here c0 is whitespace.
    let spaces = run(s, char::is_whitespace);
    // \s*[\r\n]+ takes the run up to its last line break.
    if let Some((at, c)) = s[..spaces]
        .char_indices()
        .rev()
        .find(|&(_, c)| is_newline(c))
    {
        return at + c.len_utf8();
    }
    // \s+(?!\S) leaves the last space for the word after it.
    if spaces < s.len() {
        let last = s[..spaces].chars().next_back().map_or(0, char::len_utf8);
        if spaces > last {
            return spaces - last;
        }
    }
    spaces
}

/// The pieces of `text`, in order; they join back into `text`.
pub fn split(text: &str) -> Pieces<'_> {
    Pieces { rest: text }
}

pub struct Pieces<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Pieces<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.rest.is_empty() {
            return None;
        }
        let (piece, rest) = self.rest.split_at(piece_len(self.rest));
        self.rest = rest;
        Some(piece)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn pieces(text: &str) -> Vec<&str> {
        split(text).collect()
    }

    #[test]
    fn words_punctuation_and_contractions() {
        assert_eq!(pieces("Hello world"), ["Hello", " world"]);
        assert_eq!(pieces("Hello, world!"), ["Hello", ",", " world", "!"]);
        assert_eq!(pieces("I'm here"), ["I", "'m", " here"]);
        assert_eq!(pieces("they'RE don't"), ["they", "'RE", " don", "'t"]);
        assert_eq!(pieces("'sup"), ["'s", "up"]);
        assert_eq!(pieces("'re 'x"), ["'re", " '", "x"]);
        assert_eq!(pieces("'"), ["'"]);
        assert_eq!(pieces("e.g."), ["e", ".g", "."]);
        assert_eq!(pieces("$100"), ["$", "1", "0", "0"]);
        assert_eq!(pieces("hello!!!\n\nworld"), ["hello", "!!!\n\n", "world"]);
        assert_eq!(pieces(" ... ok"), [" ...", " ok"]);
    }

    #[test]
    fn numbers_split_one_digit_at_a_time() {
        assert_eq!(pieces("12345"), ["1", "2", "3", "4", "5"]);
        assert_eq!(pieces("abc123def"), ["abc", "1", "2", "3", "def"]);
        assert_eq!(pieces("1st"), ["1", "st"]);
        // A Roman numeral is a number, not a letter; so are other scripts' digits.
        assert_eq!(pieces("\u{216b}x"), ["\u{216b}", "x"]);
        assert_eq!(
            pieces("x \u{663}\u{664} \u{b2}"),
            ["x", " ", "\u{663}", "\u{664}", " ", "\u{b2}"]
        );
    }

    #[test]
    fn whitespace_runs() {
        assert_eq!(pieces("  leading"), [" ", " leading"]);
        assert_eq!(pieces("trailing  "), ["trailing", "  "]);
        assert_eq!(pieces("a  b"), ["a", " ", " b"]);
        assert_eq!(pieces("a\n\nb"), ["a", "\n\n", "b"]);
        assert_eq!(pieces("a \n b"), ["a", " \n", " b"]);
        assert_eq!(pieces("x  \n\n  y"), ["x", "  \n\n", " ", " y"]);
        assert_eq!(pieces("a\r\nb"), ["a", "\r\n", "b"]);
        assert_eq!(pieces("  \n"), ["  \n"]);
        assert_eq!(pieces("\t\tfoo"), ["\t", "\tfoo"]);
        assert_eq!(pieces("x\u{a0}y"), ["x", "\u{a0}y"]);
        assert_eq!(pieces("hello\n"), ["hello", "\n"]);
        assert_eq!(pieces(" "), [" "]);
        // A lone CR is a line break; VT, FF, U+0085 and U+2028 are whitespace
        // but not [\r\n], so they lead the next word.
        assert_eq!(pieces("a\rb"), ["a", "\r", "b"]);
        assert_eq!(pieces("a\u{b}\u{c}b"), ["a", "\u{b}", "\u{c}b"]);
        assert_eq!(pieces("a\u{2028}b"), ["a", "\u{2028}b"]);
        assert_eq!(pieces("a\u{85}\n"), ["a", "\u{85}\n"]);
        assert_eq!(pieces("don'"), ["don", "'"]);
    }

    #[test]
    fn marks_and_symbols_unicode_calls_alphabetic_are_not_letters() {
        // A circled letter is a symbol; Thai and Devanagari vowel signs are marks.
        assert_eq!(pieces("a\u{24b6}b"), ["a", "\u{24b6}b"]);
        assert_eq!(
            pieces("\u{e2a}\u{e27}\u{e31}\u{e2a}\u{e14}\u{e35}\u{e04}\u{e23}\u{e31}\u{e1a}"),
            [
                "\u{e2a}\u{e27}",
                "\u{e31}\u{e2a}\u{e14}",
                "\u{e35}\u{e04}\u{e23}",
                "\u{e31}\u{e1a}"
            ]
        );
        assert_eq!(
            pieces("\u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947}"),
            ["\u{928}\u{92e}\u{938}", "\u{94d}\u{924}", "\u{947}"]
        );
    }

    #[test]
    fn other_scripts() {
        assert_eq!(pieces("caf\u{e9} na\u{ef}ve"), ["caf\u{e9}", " na\u{ef}ve"]);
        assert_eq!(
            pieces("\u{65e5}\u{672c}\u{8a9e}"),
            ["\u{65e5}\u{672c}\u{8a9e}"]
        );
        assert_eq!(pieces("\u{1f600} hi"), ["\u{1f600}", " hi"]);
    }

    #[test]
    fn pieces_join_back_into_the_text() {
        let text = "It's 2026: caf\u{e9}s, \u{1f600}!!  \r\n\t x\u{a0}\u{3000}y 'll\n";
        assert_eq!(pieces(text).concat(), text);
        assert!(pieces(text).iter().all(|p| !p.is_empty()));
        assert!(pieces("").is_empty());
    }
}
