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
    /// (app, its answer's `version` line) of the last request's apps, sorted
    /// by app (`apps::key`).
    key: Vec<(String, String)>,
}

impl Apps {
    /// A new grammar for `wire` (an `act` request's `apps`), or `None` when
    /// its apps are those of the grammar in use. `count` is the model's
    /// tokenizer: the apps turn is held to its budget in real tokens.
    pub fn update(
        &mut self,
        wire: &str,
        count: &dyn Fn(&str) -> usize,
    ) -> Option<local::act::Grammar> {
        let Some(mut entries) = apps::entries(wire) else {
            println!("llm: the request's apps are malformed; keeping the last");
            return None;
        };
        // By name: the shell lists running apps first, so the same apps come in
        // another order as one starts or stops. Sorted, the apps turn (and the
        // act slot's cached prefix) is the same for the same apps however they
        // came, and the apps past the budget are the last by name.
        entries.sort_by_key(|(app, _)| *app);
        // In any order: the same set of (app, version) keeps the grammar.
        let key = apps::key(&entries);
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
            let rows = self.rows.rows_of(app);
            // One app's rows the decoder cannot steer leave that app out, not
            // every app (audit, lane M items 3 and 5).
            if !local::act::decodable(&rows) {
                println!(
                    "llm: {app}'s rows have an argument the model cannot be held to; left out"
                );
                continue;
            }
            groups.push(AppGroup {
                app: String::from(*app),
                rows,
            });
        }
        // These apps are tried once: a failure is not built, and leaked, again
        // on every request.
        self.key = key;
        let grammar = match local::act::grammar_counted(&groups, count) {
            Ok(grammar) => grammar,
            Err(error) => {
                println!("llm: the apps' rows do not decode ({error}); keeping the last");
                return None;
            }
        };
        let (_, left_out) = apps::turn_counted(&groups, count);
        if !left_out.is_empty() {
            println!(
                "llm: past the apps budget, left out: {}",
                left_out.join(" ")
            );
        }
        println!("llm: {} apps in the act prompt", grammar.apps().len());
        Some(grammar)
    }
}
