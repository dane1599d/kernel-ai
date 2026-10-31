//! Installed apps' rows as the shell sends them with each `act` request (P4
//! task 15): checked by the shell's own loader (`shellcore::apps`), and turned
//! into the grammar's app groups (`llmproto::pack::apps`). A version already
//! loaded is not parsed or leaked again, and the grammar is rebuilt only when
//! the set of (app, version) changes.

use alloc::string::String;
use alloc::vec::Vec;
use llmproto::local;
use llmproto::pack::apps::{self, AppGroup};
use shellcore::apps::AppRows;
use user::println;

#[derive(Default)]
pub struct Apps {
    rows: AppRows,
    /// (app, its answer's `version` line) of the last request's apps.
    key: Vec<(String, String)>,
}

impl Apps {
    /// A new grammar for `wire` (an `act` request's `apps`), or `None` when
    /// its apps are those of the grammar in use.
    pub fn update(&mut self, wire: &str) -> Option<local::act::Grammar> {
        let Some(entries) = apps::entries(wire) else {
            println!("llm: the request's apps are malformed; keeping the last");
            return None;
        };
        let key: Vec<(String, String)> = entries
            .iter()
            .map(|(app, answer)| {
                let head = answer.lines().next().unwrap_or("");
                (String::from(*app), String::from(head))
            })
            .collect();
        if key == self.key {
            return None;
        }
        let names: Vec<String> = entries.iter().map(|(app, _)| String::from(*app)).collect();
        let mut groups = Vec::new();
        for (app, answer) in &entries {
            let others: Vec<String> = names.iter().filter(|n| n != app).cloned().collect();
            if let Err(why) = self.rows.load_among(app, answer, &others) {
                println!("llm: {why}");
                continue;
            }
            groups.push(AppGroup {
                app: String::from(*app),
                rows: self.rows.rows_of(app),
            });
        }
        let grammar = match local::act::grammar_with(&groups) {
            Ok(grammar) => grammar,
            Err(error) => {
                println!("llm: the apps' rows do not decode ({error}); keeping the last");
                return None;
            }
        };
        let (_, left_out) = apps::turn(&groups);
        if !left_out.is_empty() {
            println!(
                "llm: past the apps budget, left out: {}",
                left_out.join(" ")
            );
        }
        self.key = key;
        println!("llm: {} apps in the act prompt", grammar.apps().len());
        Some(grammar)
    }
}
