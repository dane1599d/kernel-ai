//! The script log: the last [`KEEP`] LLM requests, what was proposed and what became
//! of it. Each entry is also written to the console when it happens.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use core::fmt::Write;

const KEEP: usize = 32;
const REQUEST_CHARS: usize = 60;

pub struct Entry {
    pub id: u64,
    pub request: String,
    pub script: String,
    pub outcome: String,
}

#[derive(Default)]
pub struct Log {
    entries: VecDeque<Entry>,
}

impl Log {
    pub fn record(&mut self, id: u64, request: &str, script: &str, outcome: &str) {
        if self.entries.len() == KEEP {
            self.entries.pop_front();
        }
        let request = crate::show::truncate(request, REQUEST_CHARS);
        self.entries.push_back(Entry {
            id,
            request,
            script: String::from(script),
            outcome: String::from(outcome),
        });
    }

    /// One line per entry, oldest first.
    pub fn list(&self) -> String {
        if self.entries.is_empty() {
            return String::from("no LLM scripts yet");
        }
        let mut text = String::new();
        for entry in &self.entries {
            let _ = writeln!(text, "#{} {}: {}", entry.id, entry.request, entry.outcome);
        }
        String::from(text.trim_end())
    }

    /// The script of entry `id`.
    pub fn script(&self, id: u64) -> Option<String> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.id == id && !entry.script.is_empty())
            .map(|entry| format!("script {}:\n{}", entry.id, entry.script))
    }
}
