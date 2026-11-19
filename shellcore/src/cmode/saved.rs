//! The live definitions of a `c>` session as one C file, for `:save`.
//!
//! Each defining entry is split into its items with promptcore's own
//! splitter, the one cc uses. An item claims names as cc's session does
//! (`promptcore::defs`): a function's or a variable's name, `tag Point`,
//! `#N` for a macro, `<h>` for an include. An item is live when no later
//! item claims one of its names, so a redefined function is written once,
//! its last body, and the file loads and includes as one entry. Every live
//! function's prototype goes first, so a body may call one written after it,
//! as at the prompt, where a redefinition goes last. Statements define
//! nothing and are never written.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use promptcore::prepare::BASE_TYPES;
use promptcore::split::{Storage, What, split};

/// One item of a defining entry: the names it claims and its text.
struct Item {
    keys: Vec<String>,
    text: String,
    /// A function's prototype, written before every body.
    proto: Option<String>,
}

/// The names `what` claims; a typedef's names are added to `types`.
fn keys_of(what: &What, types: &mut Vec<String>) -> Vec<String> {
    match what {
        What::Define(name) | What::Undef(name) => alloc::vec![format!("#{name}")],
        What::Include(header) => alloc::vec![String::from(header.as_str())],
        What::Func(func) => alloc::vec![func.name.clone()],
        What::Decl(decl) => {
            let names = decl.declarators.iter().map(|d| d.name.clone());
            if decl.storage == Storage::Typedef {
                types.extend(names.clone());
            }
            let tags = decl.tags.iter().map(|t| format!("tag {}", t.name));
            let constants = decl
                .tags
                .iter()
                .flat_map(|t| t.enumerators.iter().cloned())
                .chain(decl.enumerators.iter().cloned());
            names.chain(tags).chain(constants).collect()
        }
        What::Stmt { .. } => Vec::new(),
    }
}

/// `entry`'s items that define something, in order. An entry that does not
/// split (cc took it, so this should not happen) is kept whole, claiming
/// nothing.
fn items_of(entry: &str, types: &mut Vec<String>) -> Vec<Item> {
    let Ok(tokens) = promptcore::lex::lex(entry) else {
        return alloc::vec![whole(entry)];
    };
    let known = types.clone();
    let is_type = |name: &str| BASE_TYPES.contains(&name) || known.iter().any(|t| t == name);
    let Ok(items) = split(entry, &tokens, &is_type) else {
        return alloc::vec![whole(entry)];
    };
    items
        .iter()
        .filter_map(|item| {
            let keys = keys_of(&item.what, types);
            // `#undef` claims its macro so the define goes, and writes nothing.
            let undef = matches!(item.what, What::Undef(_));
            let proto = match &item.what {
                What::Func(func) if !func.local => Some(func.proto.clone()),
                _ => None,
            };
            (!keys.is_empty()).then(|| Item {
                keys,
                proto,
                text: match undef {
                    true => String::new(),
                    false => String::from(&entry[item.start..item.end]),
                },
            })
        })
        .collect()
}

fn whole(entry: &str) -> Item {
    Item {
        keys: Vec::new(),
        text: String::from(entry.trim_end()),
        proto: None,
    }
}

/// The live functions' prototypes, then the text of every live definition
/// in `entries` (oldest first) in the order it ran, `header` left out where
/// an entry included it again.
pub(super) fn live(entries: &[&str], header: &str) -> (Vec<String>, Vec<String>) {
    let mut types = Vec::new();
    let items: Vec<Item> = entries
        .iter()
        .flat_map(|entry| items_of(entry, &mut types))
        .collect();
    let live: Vec<&Item> = items
        .iter()
        .enumerate()
        .filter(|(i, item)| {
            let later = &items[i + 1..];
            let replaced = item
                .keys
                .iter()
                .any(|key| later.iter().any(|l| l.keys.contains(key)));
            !replaced && !item.text.is_empty() && item.keys.iter().all(|k| k != header)
        })
        .map(|(_, item)| item)
        .collect();
    let protos = live.iter().filter_map(|item| item.proto.clone()).collect();
    let bodies = live.iter().map(|item| item.text.clone()).collect();
    (protos, bodies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redefinition_is_written_once_its_last_body() {
        let entries = [
            "I64 Sq(I64 x) { return x * x; }",
            "I64 Twice(I64 x) { return Sq(x) * 2; }\nTwice(3);",
            "I64 Sq(I64 x) { return x + 1; }",
        ];
        let (protos, bodies) = live(&entries, "<kai.h>");
        assert_eq!(protos, ["I64 Twice(I64 x);", "I64 Sq(I64 x);"]);
        assert_eq!(
            bodies,
            [
                "I64 Twice(I64 x) { return Sq(x) * 2; }",
                "I64 Sq(I64 x) { return x + 1; }"
            ]
        );
    }

    #[test]
    fn typedefs_macros_and_tags_are_tracked() {
        let entries = [
            "#include <kai.h>\ntypedef I64 Money;\nMoney Cost = 3;",
            "#define N 4\nstruct P { I64 x; };",
            "#undef N\nMoney Cost = 5;",
        ];
        assert_eq!(
            live(&entries, "<kai.h>").1,
            [
                "typedef I64 Money;",
                "struct P { I64 x; };",
                "Money Cost = 5;"
            ]
        );
    }
}
