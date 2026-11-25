//! A saved `c>` session's file (`:save`, `:load`, `#include "…"`).
//!
//! `:save` writes each defining entry as it was typed, in the order it ran,
//! each after a line [`MARK`] that code writes, so `:load` replays the
//! entries one at a time exactly as the session ran them: a redefinition
//! replaces as it did when typed. The mark is a C comment, so the file is
//! plain C, and `#include` reads it as one unit; one unit cannot define a
//! name twice, so an include of a file that does is refused by name
//! ([`redefined`]) and `:load` is the way in.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use promptcore::prepare::BASE_TYPES;
use promptcore::split::{Storage, What, split};

/// The line before each entry of a saved file.
pub(super) const MARK: &str = "//- entry";
/// What a saved file starts with, which a `c>` session has already.
pub(super) const HEADER: &str = "#include <kai.h>";

/// A saved file's text for `entries`, or why not: an entry holds a line
/// that is the mark itself, which would cut it in two on `:load`.
pub(super) fn file(entries: &[&str]) -> Result<String, String> {
    let mut text = format!("{HEADER}\n");
    for entry in entries {
        if entry.lines().any(|line| line.trim() == MARK) {
            return Err(format!("an entry holds a line `{MARK}`, which :save uses"));
        }
        text.push('\n');
        text.push_str(MARK);
        text.push('\n');
        text.push_str(entry.trim_end());
        text.push('\n');
    }
    Ok(text)
}

/// The entries of a file, cut at [`MARK`] lines, without the blank ones and
/// the lone [`HEADER`] before the first mark. A file without marks is one
/// entry.
pub(super) fn entries(text: &str) -> Vec<String> {
    let mut out: Vec<String> = alloc::vec![String::new()];
    for line in text.split_inclusive('\n') {
        if line.trim() == MARK {
            out.push(String::new());
        } else if let Some(entry) = out.last_mut() {
            entry.push_str(line);
        }
    }
    out.into_iter()
        .map(|entry| String::from(entry.trim_end()))
        .filter(|entry| !entry.trim().is_empty() && entry.trim() != HEADER)
        .collect()
}

/// The first name `text` defines twice, as one unit would: a function with
/// its body, a variable with its initializer, a tag with its body, an enum
/// constant or a macro (an `#undef` lets a macro be defined again). A
/// prototype, an `extern`, a typedef or a variable without an initializer
/// defines nothing here, as C lets them repeat. `None` when there is none, or when
/// `text` does not split (cc then says why).
pub(super) fn redefined(text: &str) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for claim in claims(text)? {
        match claim {
            Claim::Undef(key) => seen.retain(|k| *k != key),
            Claim::Defines(keys) => {
                for key in keys {
                    if seen.contains(&key) {
                        return Some(key);
                    }
                    seen.push(key);
                }
            }
        }
    }
    None
}

/// Every name `text` defines, by [`redefined`]'s rule, each once: a macro
/// as `#NAME`, a tag as `tag NAME`. None when `text` does not split.
pub(super) fn defines(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for claim in claims(text).unwrap_or_default() {
        if let Claim::Defines(keys) = claim {
            for key in keys {
                if !names.contains(&key) {
                    names.push(key);
                }
            }
        }
    }
    names
}

/// Each of `names` (as [`defines`] gives them, as keys) that `text`'s code
/// names, outside comments, strings and the names `text` defines itself.
/// It is a match on the word, so a parameter or local of that name counts.
pub(super) fn mentions(text: &str, names: &[String]) -> Vec<String> {
    let own = defines(text);
    let Ok(tokens) = promptcore::lex::lex(text) else {
        return Vec::new();
    };
    names
        .iter()
        .filter(|key| !own.contains(key))
        .filter(|key| {
            tokens
                .iter()
                .filter(|t| t.kind == promptcore::lex::Kind::Ident)
                .any(|t| t.text(text) == bare(key))
        })
        .cloned()
        .collect()
}

/// A key as the code writes it: `#N` and `tag P` are `N` and `P`.
pub(super) fn bare(key: &str) -> &str {
    key.strip_prefix('#')
        .or_else(|| key.strip_prefix("tag "))
        .unwrap_or(key)
}

/// What one split item of an entry does to the names it defines.
enum Claim {
    /// `#undef`: the macro may be defined again.
    Undef(String),
    Defines(Vec<String>),
}

/// The claims of `text`'s items in order; `None` when it does not split.
fn claims(text: &str) -> Option<Vec<Claim>> {
    let tokens = promptcore::lex::lex(text).ok()?;
    let is_type = |name: &str| BASE_TYPES.contains(&name);
    let items = split(text, &tokens, &is_type).ok()?;
    let claims = items
        .iter()
        .filter_map(|item| match &item.what {
            What::Undef(name) => Some(Claim::Undef(format!("#{name}"))),
            What::Define(name) => Some(Claim::Defines(alloc::vec![format!("#{name}")])),
            What::Func(func) => Some(Claim::Defines(alloc::vec![func.name.clone()])),
            What::Decl(decl) => Some(Claim::Defines(
                decl.declarators
                    .iter()
                    .filter(|d| d.init.is_some() && decl.storage != Storage::Extern)
                    .map(|d| d.name.clone())
                    .chain(decl.tags.iter().map(|t| format!("tag {}", t.name)))
                    .chain(decl.tags.iter().flat_map(|t| t.enumerators.clone()))
                    .chain(decl.enumerators.iter().cloned())
                    .collect(),
            )),
            What::Include(_) | What::Stmt { .. } => None,
        })
        .collect();
    Some(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_cuts_back_into_its_entries() {
        let typed = ["I64 Sq(I64 x) { return x * x; }", "I64 lo, hi;\nhi = 2;"];
        let text = file(&typed).unwrap();
        assert!(
            text.starts_with("#include <kai.h>\n\n//- entry\n"),
            "{text}"
        );
        assert_eq!(entries(&text), typed);
        assert_eq!(
            entries("I64 One() { return 1; }\n"),
            ["I64 One() { return 1; }"]
        );
        assert!(file(&["I64 a;\n//- entry\nI64 b;"]).is_err());
    }

    #[test]
    fn a_name_defined_twice_is_found() {
        let twice = "I64 Sq(I64 x) { return x; }\n//- entry\nI64 Sq(I64 x) { return 1; }";
        assert_eq!(redefined(twice).as_deref(), Some("Sq"));
        let once = "typedef struct { I64 x; } Point;\nPoint Add(Point a) { return a; }";
        assert_eq!(redefined(once), None);
        assert_eq!(
            redefined("#define N 2\n#undef N\n#define N 3\nI64 a[N];"),
            None
        );
        assert_eq!(
            redefined("I64 lo = 1, hi = 2;\nI64 hi = 9;").as_deref(),
            Some("hi")
        );
        let declared = "I64 Sq(I64 x);\nextern I64 n;\nI64 Sq(I64 x) { return x; }\nI64 n = 2;";
        assert_eq!(redefined(declared), None);
    }

    #[test]
    fn an_entry_uses_a_name_another_defines_but_not_its_own() {
        let lib = "I64 Seven() { return 70; }\n#define N 3\nstruct P { I64 x; };";
        assert_eq!(defines(lib), ["Seven", "#N", "tag P"]);
        let names = defines(lib);
        let eight = "I64 Eight() { return Seven() + 1; }";
        assert_eq!(mentions(eight, &names), ["Seven"]);
        assert_eq!(mentions("I64 a[N] = Seven();", &names), ["Seven", "#N"]);
        // A comment or a string is no use; a redefinition is the entry's own.
        assert!(mentions("/* Seven */ U8 *s = \"Seven\";", &names).is_empty());
        assert!(mentions("I64 Seven() { return 7; }", &names).is_empty());
        assert!(mentions("I64 Nine() { return 9; }", &names).is_empty());
    }
}
