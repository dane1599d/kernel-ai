//! The live definitions of one session and their undo journal (ADR 0012 D4).
//!
//! Each definition claims names in C's namespaces: `x` for an ordinary
//! identifier (variables, functions, typedefs and enum constants share one),
//! `tag Point` for a `struct`, `union` or `enum` tag, `#N` for a macro and
//! `<stdio.h>` for an include. A new definition replaces every older one that
//! claims one of its names, and goes last, after everything it may use.
//!
//! Every committed entry leaves one journal record, even one that defined
//! nothing, so `undo` always reverts the last entry: it drops what that entry
//! added and puts back what it replaced, where it was.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// Most definitions a session holds: one per table slot at most (kailink's
/// default `table_limit`).
pub const MAX_DEFS: usize = 4096;
/// Most entries `undo` can go back.
pub const MAX_UNDO: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Include,
    Macro,
    Type,
    Typedef,
    Var,
    Func,
    /// A declaration of something defined elsewhere: a prototype, an `extern`.
    Decl,
}

impl Kind {
    /// The word `def` records and `:defs` use.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Include => "include",
            Kind::Macro => "macro",
            Kind::Type => "type",
            Kind::Typedef => "typedef",
            Kind::Var => "var",
            Kind::Func => "fn",
            Kind::Decl => "decl",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    /// The names it claims (see the module header).
    pub keys: Vec<String>,
    pub kind: Kind,
    /// How `:defs` and replies name it: `Sq`, `struct Point`, `#define N`.
    pub name: String,
    /// What later entries' preludes hold, ending in a newline.
    pub text: String,
    /// A function that takes no arguments, so `Name;` calls it.
    pub nullary: bool,
    /// The entry that made it.
    pub seq: u64,
}

impl Def {
    fn claims_any(&self, keys: &[String]) -> bool {
        self.keys.iter().any(|k| keys.contains(k))
    }
}

/// What one committed entry changed.
#[derive(Clone, Debug)]
struct Change {
    seq: u64,
    added: usize,
    /// Definitions it replaced, with where each was, in rising order.
    removed: Vec<(usize, Def)>,
}

/// What `undo` reverted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undone {
    pub seq: u64,
    /// What the entry had defined, now gone.
    pub dropped: Vec<Def>,
    /// What it had replaced, now back.
    pub restored: Vec<Def>,
}

#[derive(Clone, Debug, Default)]
pub struct Defs {
    items: Vec<Def>,
    journal: VecDeque<Change>,
}

impl Defs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The definitions, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Def> {
        self.items.iter()
    }

    /// The definition claiming `key`.
    pub fn get(&self, key: &str) -> Option<&Def> {
        self.items.iter().find(|d| d.keys.iter().any(|k| k == key))
    }

    /// Whether `name` is a typedef of this session.
    pub fn is_type(&self, name: &str) -> bool {
        self.get(name).is_some_and(|d| d.kind == Kind::Typedef)
    }

    /// The function definition named `name`.
    pub fn func(&self, name: &str) -> Option<&Def> {
        self.get(name).filter(|d| d.kind == Kind::Func)
    }

    /// The text of every definition that claims none of `except`, in order.
    pub fn prelude(&self, except: &[String]) -> String {
        let mut text = String::new();
        for def in self.items.iter().filter(|d| !d.claims_any(except)) {
            text.push_str(&def.text);
        }
        text
    }

    /// Records entry `seq`'s definitions, and `drops`, the macros it undefined.
    pub fn commit(&mut self, seq: u64, defs: Vec<Def>, drops: &[String]) {
        let mut claimed: Vec<String> = drops.to_vec();
        for def in &defs {
            claimed.extend(def.keys.iter().cloned());
        }
        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(self.items.len() + defs.len());
        for (at, def) in core::mem::take(&mut self.items).into_iter().enumerate() {
            match def.claims_any(&claimed) {
                true => removed.push((at, def)),
                false => kept.push(def),
            }
        }
        self.items = kept;
        let added = defs.len();
        self.items.extend(defs);
        if self.journal.len() == MAX_UNDO {
            self.journal.pop_front();
        }
        self.journal.push_back(Change {
            seq,
            added,
            removed,
        });
    }

    /// Reverts the last committed entry; `None` when there is none left.
    pub fn undo(&mut self) -> Option<Undone> {
        let change = self.journal.pop_back()?;
        let keep = self.items.len() - change.added;
        let dropped = self.items.split_off(keep);
        let mut restored = Vec::with_capacity(change.removed.len());
        for (at, def) in change.removed {
            restored.push(def.clone());
            self.items.insert(at.min(self.items.len()), def);
        }
        Some(Undone {
            seq: change.seq,
            dropped,
            restored,
        })
    }

    /// Whether `undo` has an entry to revert.
    pub fn can_undo(&self) -> bool {
        !self.journal.is_empty()
    }

    /// Forgets everything: `:reset`.
    pub fn clear(&mut self) {
        self.items.clear();
        self.journal.clear();
    }

    /// `:defs`: one line per definition, its kind and its first line of text.
    pub fn listing(&self) -> String {
        let mut out = String::new();
        for def in &self.items {
            let text = def.text.trim_end();
            let text = text.strip_prefix("extern ").unwrap_or(text);
            let (first, more) = match text.split_once('\n') {
                Some((first, _)) => (first, " …"),
                None => (text, ""),
            };
            let _ = writeln!(out, "{:<7} {first}{more}", def.kind.word());
        }
        out
    }
}
