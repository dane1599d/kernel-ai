//! Settings' long lists, a page an answer. One answer is at most
//! [`crate::settings::MAX_ANSWER`] bytes, and `net` and `rules` can list more
//! (256 grants; 32 rules and 32 instructions): such an answer ended in an
//! error, and the page said nothing was granted. A page is whole lines from
//! line `skip`, and, when lines are left, a last line `more <n>`: the `skip` of
//! the page after it.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What the `more <n>` line can take: `more ` and nine digits and its newline.
const MORE: usize = "\nmore 999999999".len();

/// The longest line a page holds, so that at least one fits with `more`.
pub(crate) const MAX_LINE: usize = 512;
const _: () = assert!(MAX_LINE + MORE < crate::settings::MAX_ANSWER);

/// `skip` in `net <skip>` or `rules <skip>`: digits, nine at most.
pub(crate) fn skip_of(word: &str) -> Option<usize> {
    let ok = !word.is_empty() && word.len() <= 9 && word.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| word.parse().ok()).flatten()
}

/// The page of `lines` from line `skip`.
pub(crate) fn page(lines: &[String], skip: usize) -> String {
    let budget = crate::settings::MAX_ANSWER - MORE;
    let mut text = String::new();
    let mut taken = 0;
    for line in lines.iter().skip(skip) {
        let line = cut(line);
        if !text.is_empty() && text.len() + 1 + line.len() > budget {
            break;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(line);
        taken += 1;
    }
    let next = skip + taken;
    if next < lines.len() {
        text.push_str(&format!("\nmore {next}"));
    }
    text
}

/// `line` cut to [`MAX_LINE`] bytes, at a character.
fn cut(line: &str) -> &str {
    let mut end = line.len().min(MAX_LINE);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

/// `text`'s lines, as `page` takes them.
pub(crate) fn lines_of(text: &str) -> Vec<String> {
    text.lines().map(String::from).collect()
}
