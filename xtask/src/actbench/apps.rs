//! The shipped apps' groups (P4 task 15): the `kai-actions 1` sections the
//! image's apps carry, written from their WIT as `components::pack` writes
//! them and loaded through the shell's own loader, so the arms that measure
//! what ships (`act`, `os`, `view`) see the apps turn llm shows.

use llmproto::local::act::{self as local_act, Grammar};
use llmproto::pack::apps::AppGroup;
use shellcore::apps::AppRows;

/// Each shipped app's groups, in the image's order: the last version of each
/// component in `components::VERSIONS`.
pub fn shipped() -> Result<Vec<AppGroup>, String> {
    let root = crate::workspace_root();
    let mut latest: Vec<&crate::components::Version> = Vec::new();
    for version in crate::components::VERSIONS {
        latest.retain(|v| v.recipe.component != version.recipe.component);
        latest.push(version);
    }
    let names: Vec<String> = latest.iter().map(|v| v.recipe.component.into()).collect();
    let mut rows = AppRows::default();
    let mut groups = Vec::new();
    for version in latest {
        let app = version.recipe.component;
        let section = crate::components::actions(&root, &version.recipe)?;
        let answer = format!("version {}\n{section}", version.label);
        let others: Vec<String> = names.iter().filter(|n| *n != app).cloned().collect();
        // As in the OS: an app whose rows the shell refuses shows none.
        if let Err(why) = rows.load_among(app, &answer, &others) {
            eprintln!("actbench: shipped app {app} shows no rows: {why}");
            continue;
        }
        groups.push(AppGroup {
            app: app.into(),
            rows: rows.rows_of(app),
        });
    }
    Ok(groups)
}

/// The grammar llm builds with the shipped apps.
pub fn grammar() -> Result<Grammar, String> {
    local_act::grammar_with(&shipped()?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmproto::pack::apps;

    #[test]
    fn the_shipped_apps_rows_load_and_fit_the_apps_budget() {
        let groups = shipped().unwrap();
        let (turn, left_out) = apps::turn(&groups);
        println!("{turn}");
        for group in &groups {
            println!("{}", apps::group_text(group));
        }
        assert!(left_out.is_empty(), "{left_out:?}");
        assert!(llmproto::pack::estimate_tokens(&turn) <= apps::APPS_BUDGET);
        assert!(grammar().is_ok());
    }
}
