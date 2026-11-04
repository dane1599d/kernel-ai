//! The memories, and forgetting that really forgets: `forget` can be undone
//! for [`FORGET_UNDO_NS`], then [`Memories::purge`] drops the record for good.
//! The `memory` volume keeps its snapshots for one hour, so a purged memory is
//! gone from them within the hour too (ADR 0013 §3).

use crate::record::{self, Memory, Scope, text_problem};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// How long a forgotten memory can be brought back: 10 minutes.
pub const FORGET_UNDO_NS: u64 = 10 * 60 * 1_000_000_000;
/// The most memories kept (est.): a store this full refuses new ones until
/// some are forgotten.
pub const MAX_MEMORIES: usize = 512;

/// Who is asking: the shell (the user) sees every memory, an agent its own
/// and those for all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader<'a> {
    User,
    Agent(&'a str),
}

impl Reader<'_> {
    fn sees(&self, scope: &Scope) -> bool {
        match (self, scope) {
            (Reader::User, _) | (_, Scope::All) => true,
            (Reader::Agent(name), Scope::Agent(own)) => *name == own,
        }
    }
}

/// The memories, and those forgotten within the undo window.
#[derive(Clone, Debug, Default)]
pub struct Memories {
    kept: Vec<Memory>,
    /// Each with when it was forgotten.
    forgotten: Vec<(Memory, u64)>,
    next: u64,
}

impl Memories {
    pub fn new() -> Self {
        Self::default()
    }

    /// The store a `kai-memory 1` file holds.
    pub fn load(file: &str) -> Result<Self, String> {
        let kept = record::decode(file)?;
        let next = kept.iter().map(|m| m.id).max().unwrap_or(0);
        Ok(Self {
            kept,
            forgotten: Vec::new(),
            next,
        })
    }

    /// The memories kept, as a `kai-memory 1` file. Forgotten ones are not
    /// in it, so a purge needs no rewrite.
    pub fn save(&self) -> String {
        record::encode(&self.kept)
    }

    /// Keeps `text` for `scope`: its new id. A text already kept for that
    /// scope gives the id it has.
    pub fn remember(
        &mut self,
        text: &str,
        scope: Scope,
        source: u64,
        time: u64,
    ) -> Result<u64, String> {
        let text = text.trim();
        if let Some(why) = text_problem(text) {
            return Err(String::from(why));
        }
        if let Some(same) = self
            .kept
            .iter()
            .find(|m| m.scope == scope && m.text.eq_ignore_ascii_case(text))
        {
            return Ok(same.id);
        }
        if self.kept.len() >= MAX_MEMORIES {
            return Err(format!(
                "Kai keeps at most {MAX_MEMORIES} memories; forget some first"
            ));
        }
        self.next += 1;
        self.kept.push(Memory {
            id: self.next,
            time,
            source,
            scope,
            text: text.to_string(),
        });
        Ok(self.next)
    }

    pub fn get(&self, id: u64) -> Option<&Memory> {
        self.kept.iter().find(|m| m.id == id)
    }

    /// What `reader` may see, newest first.
    pub fn list(&self, reader: Reader) -> Vec<&Memory> {
        let mut seen: Vec<&Memory> = self.kept.iter().filter(|m| reader.sees(&m.scope)).collect();
        seen.sort_by(|a, b| b.time.cmp(&a.time).then(b.id.cmp(&a.id)));
        seen
    }

    /// What `reader` may see whose text has every word of `query`, newest
    /// first.
    pub fn search(&self, reader: Reader, query: &str) -> Vec<&Memory> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        self.list(reader)
            .into_iter()
            .filter(|m| {
                let text = m.text.to_ascii_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect()
    }

    /// Changes memory `id`'s text: the text it had, for undo.
    pub fn edit(&mut self, id: u64, text: &str, time: u64) -> Result<String, String> {
        let text = text.trim();
        if let Some(why) = text_problem(text) {
            return Err(String::from(why));
        }
        let memory = self
            .kept
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| format!("no memory {id}"))?;
        memory.time = time;
        Ok(core::mem::replace(&mut memory.text, text.to_string()))
    }

    /// Forgets memory `id`; [`Self::restore`] brings it back within
    /// [`FORGET_UNDO_NS`].
    pub fn forget(&mut self, id: u64, now: u64) -> Result<(), String> {
        let at = self
            .kept
            .iter()
            .position(|m| m.id == id)
            .ok_or_else(|| format!("no memory {id}"))?;
        let memory = self.kept.remove(at);
        self.forgotten.push((memory, now));
        Ok(())
    }

    /// Forgets every memory: how many.
    pub fn forget_all(&mut self, now: u64) -> usize {
        let count = self.kept.len();
        self.forgotten
            .extend(self.kept.drain(..).map(|memory| (memory, now)));
        count
    }

    /// Undoes `forget` of `id`, if it is within the window.
    pub fn restore(&mut self, id: u64, now: u64) -> Result<(), String> {
        self.purge(now);
        let at = self
            .forgotten
            .iter()
            .position(|(m, _)| m.id == id)
            .ok_or_else(|| format!("memory {id} was forgotten more than 10 minutes ago"))?;
        let (memory, _) = self.forgotten.remove(at);
        self.kept.push(memory);
        Ok(())
    }

    /// Drops for good what was forgotten more than [`FORGET_UNDO_NS`] ago:
    /// how many.
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
