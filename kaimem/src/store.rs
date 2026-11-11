//! The memories, and forgetting that really forgets: `forget` can be undone
//! for [`FORGET_UNDO_NS`], then [`Memories::purge`] drops the record for good.
//! The `memory` volume keeps its snapshots for one hour, so a purged memory is
//! gone from them within the hour too (ADR 0013 §3).
//!
//! Every read and write names who does it ([`Who`]), and the store keeps the
//! scope rule itself (C on #650): the user reads and writes every memory; an
//! agent reads its own and those for all, and writes only its own.

use crate::record::{self, Memory, Scope, is_agent_name, text_problem};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// How long a forgotten memory can be brought back: 10 minutes.
pub const FORGET_UNDO_NS: u64 = 10 * 60 * 1_000_000_000;
/// The most memories kept (est.): a store this full refuses new ones until
/// some are forgotten.
pub const MAX_MEMORIES: usize = 512;

/// Who reads or writes: the shell (the user), or an agent by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who<'a> {
    User,
    Agent(&'a str),
}

impl Who<'_> {
    fn sees(&self, scope: &Scope) -> bool {
        match (self, scope) {
            (Who::User, _) | (_, Scope::All) => true,
            (Who::Agent(name), Scope::Agent(own)) => *name == own,
        }
    }

    fn writes(&self, scope: &Scope) -> bool {
        match (self, scope) {
            (Who::User, _) => true,
            (Who::Agent(name), Scope::Agent(own)) => *name == own,
            (Who::Agent(_), Scope::All) => false,
        }
    }
}

/// The memories, and those forgotten within the undo window.
#[derive(Clone, Debug)]
pub struct Memories {
    kept: Vec<Memory>,
    /// Each with when it was forgotten.
    forgotten: Vec<(Memory, u64)>,
    /// The id the next memory gets; never one handed out before.
    next: u64,
}

impl Default for Memories {
    fn default() -> Self {
        Self {
            kept: Vec::new(),
            forgotten: Vec::new(),
            next: 1,
        }
    }
}

impl Memories {
    pub fn new() -> Self {
        Self::default()
    }

    /// The store a `kai-memory 1` file holds.
    pub fn load(file: &str) -> Result<Self, String> {
        let (next, kept) = record::decode(file)?;
        Ok(Self {
            kept,
            forgotten: Vec::new(),
            next,
        })
    }

    /// The memories kept and the next id, as a `kai-memory 1` file. Forgotten
    /// ones are not in it, so a purge needs no rewrite.
    pub fn save(&self) -> String {
        record::encode(self.next, &self.kept)
    }

    /// Keeps `text` for `scope`, written by `who`: its new id. A text already
    /// kept for that scope gives the id it has.
    pub fn remember(
        &mut self,
        who: Who,
        text: &str,
        scope: Scope,
        source: u64,
        time: u64,
    ) -> Result<u64, String> {
        if !who.writes(&scope) {
            return Err(String::from("an agent writes only its own memories"));
        }
        // `all` is the file's word for memories that every agent sees: an agent so
        // named would be read back as everyone's, and a scope the file cannot hold
        // would make the whole file unreadable.
        if let Scope::Agent(name) = &scope
            && (!is_agent_name(name) || name == "all")
        {
            return Err(String::from(
                "a memory is for all agents or for one agent by a name it can keep",
            ));
        }
        let text = text.trim();
        if let Some(why) = text_problem(text) {
            return Err(String::from(why));
        }
        if let Some(same) = self.same(&scope, text) {
            return Ok(same.id);
        }
        if self.kept.len() >= MAX_MEMORIES {
            return Err(format!(
                "Kai keeps at most {MAX_MEMORIES} memories; forget some first"
            ));
        }
        let id = self.next;
        self.next += 1;
        self.kept.push(Memory {
            id,
            time,
            source,
            scope,
            text: text.to_string(),
        });
        Ok(id)
    }

    fn same(&self, scope: &Scope, text: &str) -> Option<&Memory> {
        self.kept
            .iter()
            .find(|m| m.scope == *scope && m.text.eq_ignore_ascii_case(text))
    }

    /// Memory `id`, if `who` may see it.
    pub fn get(&self, who: Who, id: u64) -> Option<&Memory> {
        self.kept.iter().find(|m| m.id == id && who.sees(&m.scope))
    }

    /// What `who` may see, newest first.
    pub fn list(&self, who: Who) -> Vec<&Memory> {
        let mut seen: Vec<&Memory> = self.kept.iter().filter(|m| who.sees(&m.scope)).collect();
        seen.sort_by(|a, b| b.time.cmp(&a.time).then(b.id.cmp(&a.id)));
        seen
    }

    /// What `who` may see whose text has every word of `query`, newest first.
    pub fn search(&self, who: Who, query: &str) -> Vec<&Memory> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        self.list(who)
            .into_iter()
            .filter(|m| {
                let text = m.text.to_ascii_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect()
    }

    /// The position of memory `id` if `who` may write it; one it may not see
    /// is reported as missing, so its id says nothing.
    fn writable(&self, who: Who, id: u64) -> Result<usize, String> {
        let at = self
            .kept
            .iter()
            .position(|m| m.id == id && who.sees(&m.scope))
            .ok_or_else(|| format!("no memory {id}"))?;
        match who.writes(&self.kept[at].scope) {
            true => Ok(at),
            false => Err(format!("memory {id} is for all; only the user changes it")),
        }
    }

    /// Changes memory `id`'s text: the text it had, for undo.
    pub fn edit(&mut self, who: Who, id: u64, text: &str, time: u64) -> Result<String, String> {
        let text = text.trim();
        if let Some(why) = text_problem(text) {
            return Err(String::from(why));
        }
        let at = self.writable(who, id)?;
        // Nothing is kept twice: another memory of the scope that says the same is
        // the one to keep (it is in the scope `who` can see, so naming it tells
        // nothing).
        let scope = &self.kept[at].scope;
        if let Some(same) = self
            .kept
            .iter()
            .find(|m| m.id != id && m.scope == *scope && m.text.eq_ignore_ascii_case(text))
        {
            return Err(format!("memory {} already says that", same.id));
        }
        let memory = &mut self.kept[at];
        memory.time = time;
        Ok(core::mem::replace(&mut memory.text, text.to_string()))
    }

    /// Forgets memory `id`; [`Self::restore`] brings it back within
    /// [`FORGET_UNDO_NS`].
    pub fn forget(&mut self, who: Who, id: u64, now: u64) -> Result<(), String> {
        let at = self.writable(who, id)?;
        let memory = self.kept.remove(at);
        self.forgotten.push((memory, now));
        Ok(())
    }

    /// Forgets every memory `who` may write: how many.
    pub fn forget_all(&mut self, who: Who, now: u64) -> usize {
        let (gone, kept): (Vec<Memory>, Vec<Memory>) =
            self.kept.drain(..).partition(|m| who.writes(&m.scope));
        self.kept = kept;
        let count = gone.len();
        self.forgotten
            .extend(gone.into_iter().map(|memory| (memory, now)));
        count
    }

    /// Undoes `forget` of `id`, if it is within the window. When the same text
    /// was kept again since, the two are one: the restore keeps the newer, so
    /// nothing is kept twice (C on #650).
    pub fn restore(&mut self, who: Who, id: u64, now: u64) -> Result<(), String> {
        self.purge(now);
        let at = self
            .forgotten
            .iter()
            .position(|(m, _)| m.id == id && who.writes(&m.scope))
            .ok_or_else(|| format!("memory {id} was not forgotten in the last 10 minutes"))?;
        let (back, _) = &self.forgotten[at];
        let kept_again = self.same(&back.scope, &back.text).is_some();
        if !kept_again && self.kept.len() >= MAX_MEMORIES {
            // Still forgotten, so it can be restored when there is room.
            return Err(format!(
                "Kai keeps at most {MAX_MEMORIES} memories; forget some first"
            ));
        }
        let (memory, _) = self.forgotten.remove(at);
        if !kept_again {
            self.kept.push(memory);
        }
        Ok(())
    }

    /// Drops for good what was forgotten more than [`FORGET_UNDO_NS`] ago:
    /// how many. The wiring calls it on a timer, so a forgotten text leaves
    /// memory within the window too.
    pub fn purge(&mut self, now: u64) -> usize {
        let before = self.forgotten.len();
        self.forgotten
            .retain(|(_, at)| now.saturating_sub(*at) < FORGET_UNDO_NS);
        before - self.forgotten.len()
    }

    /// How many forgotten memories can still be brought back.
    pub fn restorable(&self) -> usize {
        self.forgotten.len()
    }
}
