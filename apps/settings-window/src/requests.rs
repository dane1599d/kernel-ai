//! What a choice in the window asks the system for: one `kai:settings` request,
//! a line of text (`wit/kai-settings/settings.wit`). The window changes nothing
//! by itself: an intent is applied to the pages only when its request comes back
//! done, and comphost passes a change on only for a real key or click, so a
//! choice the AI synthesised is refused and put back by the redraw.
//!
//! | Intent | Request |
//! |---|---|
//! | `Appearance` | `appearance-set <setting> <value>`, one for each changed setting |
//! | `UseProfile` | `profile-set <name>` |
//! | `RemoveRule` | `rules-remove <n>` or `rules-remove S<n>` |
//! | `Revoke` | `grants-revoke <id>` |
//! | `KeepRunning` | `keep-running on\|off <app>` |
//! | `RestartWindow` | `restart-window <start> <end>` or `restart-window off` |
//!
//! A name (a theme, a profile, an app) is the rest of the line, so it holds
//! spaces if it must; one that is empty, over its limit or holds a control
//! character is not sent.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_settings::Intent;
use comp_settings::files::Appearance;

/// Longest theme name sent, in bytes (the host's `MAX_VALUE`).
const MAX_THEME: usize = 64;
/// Longest profile or app name sent (the host's `MAX_NAME`).
const MAX_PROFILE: usize = 24;
const MAX_APP: usize = 24;

/// What the pages change to once the request is done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    Theme(String),
    Legible(bool),
    ReducedMotion(bool),
    Intent(Intent),
}

/// One request and what it changes when it is done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub request: String,
    pub then: Applied,
}

/// A name the host takes (`wasmhost::settings::name_ok`, which a test holds this
/// to): 1 to `max` printable ASCII bytes, no space at either end and no two
/// spaces together.
fn name_ok(name: &str, max: usize) -> bool {
    !name.is_empty()
        && name.len() <= max
        && name.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && !name.starts_with(' ')
        && !name.ends_with(' ')
        && !name.contains("  ")
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// One `appearance-set` for each setting that differs from `before`; the verb
/// takes one setting at a time.
fn appearance(before: &Appearance, after: &Appearance) -> Vec<Planned> {
    let mut out = Vec::new();
    if before.theme != after.theme && name_ok(&after.theme, MAX_THEME) {
        out.push(Planned {
            request: format!("appearance-set theme {}", after.theme),
            then: Applied::Theme(after.theme.clone()),
        });
    }
    if before.legible != after.legible {
        out.push(Planned {
            request: format!("appearance-set legible {}", on(after.legible)),
            then: Applied::Legible(after.legible),
        });
    }
    if before.reduced_motion != after.reduced_motion {
        out.push(Planned {
            request: format!("appearance-set reduced_motion {}", on(after.reduced_motion)),
            then: Applied::ReducedMotion(after.reduced_motion),
        });
    }
    out
}

/// The requests `intent` asks for, given the appearance now shown; none if it
/// holds nothing the system could take.
pub fn plan(before: &Appearance, intent: &Intent) -> Vec<Planned> {
    let one = |request: String| {
        alloc::vec![Planned {
            request,
            then: Applied::Intent(intent.clone()),
        }]
    };
    match intent {
        Intent::Appearance(after) => appearance(before, after),
        Intent::UseProfile(name) if name_ok(name, MAX_PROFILE) => {
            one(format!("profile-set {name}"))
        }
        Intent::RemoveRule(rule) => one(format!("rules-remove {}", rule.word())),
        Intent::Revoke(grant) => one(format!("grants-revoke {grant}")),
        Intent::KeepRunning { app, on: keep } if name_ok(app, MAX_APP) => {
            one(format!("keep-running {} {app}", on(*keep)))
        }
        Intent::RestartWindow(Some((start, end))) => one(format!("restart-window {start} {end}")),
        Intent::RestartWindow(None) => one(String::from("restart-window off")),
        Intent::UseProfile(_) | Intent::KeepRunning { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use comp_settings::RuleRef;
    use wasmhost::settings::{Kind, classify};

    fn dark() -> Appearance {
        Appearance {
            theme: "dark".into(),
            legible: false,
            reduced_motion: false,
        }
    }

    fn requests(intent: &Intent) -> Vec<String> {
        plan(&dark(), intent)
            .into_iter()
            .map(|p| p.request)
            .collect()
    }

    #[test]
    fn each_changed_setting_is_a_request_of_its_own_and_unchanged_ones_none() {
        let after = Appearance {
            theme: "high-contrast".into(),
            legible: true,
            reduced_motion: false,
        };
        assert_eq!(
            requests(&Intent::Appearance(after)),
            [
                "appearance-set theme high-contrast",
                "appearance-set legible on"
            ]
        );
        assert!(requests(&Intent::Appearance(dark())).is_empty());
        let motion = Appearance {
            reduced_motion: true,
            ..dark()
        };
        assert_eq!(
            requests(&Intent::Appearance(motion)),
            ["appearance-set reduced_motion on"]
        );
        let planned = plan(
            &dark(),
            &Intent::Appearance(Appearance {
                theme: "light".into(),
                ..dark()
            }),
        );
        assert_eq!(planned[0].then, Applied::Theme("light".into()));
    }

    #[test]
    fn the_other_choices_are_one_request_each() {
        let cases = [
            (
                Intent::UseProfile("deep focus".into()),
                "profile-set deep focus",
            ),
            (Intent::RemoveRule(RuleRef::Rule(3)), "rules-remove 3"),
            (
                Intent::RemoveRule(RuleRef::Instruction(2)),
                "rules-remove S2",
            ),
            (Intent::Revoke(7), "grants-revoke 7"),
            (
                Intent::KeepRunning {
                    app: "notes".into(),
                    on: true,
                },
                "keep-running on notes",
            ),
            (
                Intent::KeepRunning {
                    app: "notes".into(),
                    on: false,
                },
                "keep-running off notes",
            ),
            (
                Intent::RestartWindow(Some((120, 300))),
                "restart-window 120 300",
            ),
            (Intent::RestartWindow(None), "restart-window off"),
        ];
        for (intent, want) in cases {
            let planned = plan(&dark(), &intent);
            assert_eq!(planned.len(), 1, "{intent:?}");
            assert_eq!(planned[0].request, want);
            assert_eq!(planned[0].then, Applied::Intent(intent.clone()));
        }
    }

    #[test]
    fn a_name_the_system_could_not_take_is_not_sent() {
        assert!(requests(&Intent::UseProfile(String::new())).is_empty());
        assert!(requests(&Intent::UseProfile("n".repeat(25))).is_empty());
        assert!(requests(&Intent::UseProfile("a\nrules-remove 1".into())).is_empty());
        let keep = |app: &str| Intent::KeepRunning {
            app: app.into(),
            on: true,
        };
        assert!(requests(&keep("")).is_empty());
        assert!(requests(&keep("x\ty")).is_empty());
        assert!(requests(&keep(&"a".repeat(MAX_APP + 1))).is_empty());
        let theme = |name: &str| {
            Intent::Appearance(Appearance {
                theme: name.into(),
                ..dark()
            })
        };
        assert!(requests(&theme("")).is_empty());
        assert!(requests(&theme("a\u{1b}b")).is_empty());
    }

    #[test]
    fn the_names_the_app_sends_are_the_names_the_host_takes() {
        let names = [
            "",
            " ",
            "a",
            "a b",
            "a  b",
            " a",
            "a ",
            "caf\u{e9}",
            "a\tb",
            "a\u{202e}b",
            "abcdefghijklmnopqrstuvwxy",
            "abcdefghijklmnopqrstuvwx",
        ];
        for name in names {
            assert_eq!(
                name_ok(name, MAX_PROFILE),
                wasmhost::settings::name_ok(name, wasmhost::settings::MAX_NAME),
                "{name:?}"
            );
        }
        assert_eq!(MAX_PROFILE, wasmhost::settings::MAX_NAME);
        assert_eq!(MAX_APP, wasmhost::settings::MAX_NAME);
        assert_eq!(MAX_THEME, wasmhost::settings::MAX_VALUE);
    }

    /// The host accepts every request as the kind the intent is: a change.
    #[test]
    fn every_request_is_a_change_the_host_accepts() {
        let intents = [
            Intent::Appearance(Appearance {
                theme: "light".into(),
                legible: true,
                reduced_motion: true,
            }),
            Intent::UseProfile("quiet".into()),
            Intent::RemoveRule(RuleRef::Rule(1)),
            Intent::Revoke(1),
            Intent::KeepRunning {
                app: "notes".into(),
                on: true,
            },
            Intent::RestartWindow(Some((1, 2))),
            Intent::RestartWindow(None),
        ];
        for intent in intents {
            for planned in plan(&dark(), &intent) {
                assert_eq!(
                    classify(&planned.request),
                    Ok(Kind::Change),
                    "{}",
                    planned.request
                );
            }
        }
    }
}
