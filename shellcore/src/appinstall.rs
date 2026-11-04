//! The install card ("Kai builds apps", piece 4): the app `app_build` built
//! last installs only on the person's Allow, on the trusted path's card.
//!
//! - **Asking:** a typed `install it` or `install <name>`, or the act model's
//!   `app_install(name)`, answers with the Allow / Deny menu, which the kernel
//!   posts to `desktop`'s card or asks on the console. The card shows the app's
//!   size and every interface it imports, or, when they do not fit, how many
//!   and `show uses <name>`, which lists them; and the call its `ONCE` names:
//!   `app_install("timer", "0.0.1", "<source>")`, where `<source>` begins
//!   `main.c`'s SHA-256. The menu's id is a new log entry's, so the `ONCE`
//!   names this card alone. The model's one-line summary and `show code` are
//!   in the build's Done, just before: the card holds [`MENU_TEXT`]
//!   characters, the call included.
//! - **Checking:** the ask and the Allow both refuse a build that is not the
//!   last one, one whose `main.c` changed since it was built (stale or
//!   edited), or one installed already, each with why. The ask refuses one
//!   whose artifact the host no longer holds, so its imports are known.
//! - **Installing:** `pkg`'s `local`, given the artifact's SHA-256 the build
//!   logged, installs exactly those bytes or refuses; it publishes them, signs
//!   them with the device key
//!   and installs it as a new generation, and the app starts, as any installed
//!   app does. Nothing installs without the Allow.
//! - **Undo:** `undo` switches back to the generation before, which removes the
//!   app; its source stays in `/home/user/apps/<name>/`.
//!
//! The card opens from a line the person types, not by itself: a card that
//! opened on its own would need the kernel's terminal to take it while it
//! waits for a line (decided 2026-11-03, `docs/decisions.md`).

use crate::actions::{How, Pending};
use crate::actlog::{Change, Entry, Form, State};
use crate::appbuild::{Built, artifact_line, sha256, source_line};
use crate::ccbuild::{APPS_DIR, NO_ANSWER, installed, shown, valid_name};
use crate::script::Checked;
use crate::tiers::Tier;
use crate::undo::Inverse;
use crate::{Reply, Shell};
use agentabi::consent::Decision;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use termcore::menu::{MAX_TEXT, Menu, Origin};
use termcore::once::{CallArg, call_text};

/// The longest question the trusted path's menu shows.
pub const MENU_TEXT: usize = MAX_TEXT;
/// How many hex digits of `main.c`'s SHA-256 the card's call shows. The
/// Allow is checked against the whole hash.
const SHOWN_SOURCE: usize = 16;
/// The interface prefix the card leaves out: every import an app has is `kai:`.
const KAI: &str = "kai:";

/// An install waiting on its card's answer: the build it was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Installing {
    name: String,
    version: String,
    /// `main.c`'s whole SHA-256, as the build recorded it.
    source: String,
    /// The `app_build` entry that built it.
    built_by: u64,
}

/// `app_install(name, version, source)`, as the card shows it and its `ONCE`
/// names it.
fn install_call(built: &Built) -> Option<String> {
    let source = &built.source[..SHOWN_SOURCE.min(built.source.len())];
    call_text(
        "app_install",
        &[
            CallArg::Text(&built.name),
            CallArg::Text(&built.version),
            CallArg::Text(source),
        ],
    )
}

/// What an app's imports come to on the card: `app/log`, `ui/view`, each once.
fn uses(imports: &[String]) -> Vec<&str> {
    let mut short: Vec<&str> = Vec::new();
    for import in imports {
        let name = import.strip_prefix(KAI).unwrap_or(import);
        let name = name.split('@').next().unwrap_or(name);
        if !short.contains(&name) {
            short.push(name);
        }
    }
    short
}

/// The card's question for `call`: its size, every import and the call whole;
/// when the imports do not fit, how many there are and the line that lists
/// them. It never names fewer than the app uses (C on #626).
fn question(built: &Built, uses: &[&str], call: &str) -> Option<String> {
    let size = built.built.bytes.div_ceil(1024);
    let listed = match uses.is_empty() {
        true => String::from("nothing"),
        false => uses.join(", "),
    };
    let forms = [
        format!("Install Kai's app, {size} KiB, using {listed}, with {call}?"),
        format!("Install {size} KiB using {}: {call}?", uses.join(",")),
        format!(
            "Install {size} KiB, uses {} interfaces (show uses {}): {call}?",
            uses.len(),
            built.name
        ),
    ];
    forms.into_iter().find(|form| form.len() <= MENU_TEXT)
}

impl Shell {
    /// A typed `install it`, `install <name>` for the app Kai built last, or
    /// `show code [<name>]`; `None` for any other line, such as an install of
    /// a package from the repository.
    pub(crate) fn app_install_line(&mut self, input: &str) -> Option<Reply> {
        let built = self.app_built.as_ref().map(|b| b.name.clone());
        if let Some(rest) = input.strip_prefix("show code") {
            let name = match rest.trim() {
                "" => built?,
                name if rest.starts_with(' ') && valid_name(name) => name.to_string(),
                _ => return None,
            };
            return Some(self.app_code(&name));
        }
        if let Some(rest) = input.strip_prefix("show uses") {
            let name = match rest.trim() {
                "" => built?,
                name if rest.starts_with(' ') => name.to_string(),
                _ => return None,
            };
            return Some(self.app_uses(&name));
        }
        let name = input.strip_prefix("install ")?.trim();
        match name {
            "it" => {
                let id = self.take_id();
                Some(match built {
                    Some(built) => self.app_install_ask(id, input, &built),
                    None => Reply::err("Kai has built no app to install"),
                })
            }
            _ if built.as_deref() == Some(name) => {
                let id = self.take_id();
                Some(self.app_install_ask(id, input, name))
            }
            _ => None,
        }
    }

    /// The act model's `app_install(name)`: the same card. It always asks,
    /// whatever the line said.
    pub(crate) fn app_install_call(
        &mut self,
        id: u64,
        request: &str,
        values: &[(&'static str, String)],
        _decision: Decision,
    ) -> Reply {
        let name = values
            .iter()
            .find(|(n, _)| *n == "name")
            .map(|(_, v)| v.trim())
            .unwrap_or_default()
            .to_string();
        self.app_install_ask(id, request, &name)
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The build `name` is, if it may be installed now: the last one built,
    /// not installed, and its `main.c` still what was built.
    fn installable(&self, name: &str) -> Result<Built, String> {
        let built = match &self.app_built {
            Some(built) if built.name == name => built.clone(),
            _ => {
                return Err(format!(
                    "Kai has not built an app called {name} to install: ask Kai to build it"
                ));
            }
        };
        if self.app_installed(&built) {
            return Err(format!(
                "Kai's app {name} {} is installed already: undo removes it",
                built.version
            ));
        }
        let now = self
            .host
            .app_source_read(name)
            .map_err(|why| format!("{APPS_DIR}/{name}/main.c: {}", shown(&why)))?;
        match now {
            Some(text) if sha256(&text) == built.source => Ok(built),
            Some(_) => Err(format!(
                "{APPS_DIR}/{name}/main.c was changed since Kai built {name} {}, so that build is not installed: ask Kai to build it again",
                built.version
            )),
            None => Err(format!(
                "{APPS_DIR}/{name}/main.c is gone, so Kai's build of it is not installed"
            )),
        }
    }

    /// Asks to install `name` on the trusted path's card.
    fn app_install_ask(&mut self, id: u64, request: &str, name: &str) -> Reply {
        let built = match self.installable(name) {
            Ok(built) => built,
            Err(why) => {
                self.record(id, request, "", &format!("refused: {}", shown(&why)));
                return Reply::err(why);
            }
        };
        let Some(call) = install_call(&built) else {
            return Reply::err("the install's call cannot be shown");
        };
        // The card names what the artifact cc built imports, or asks nothing:
        // the host must still hold that very artifact.
        if self.host.cc_artifact(name).as_ref() != Some(&built.artifact) {
            let why = format!(
                "what Kai's app {name} imports cannot be read, since the artifact cc built for it is no longer the last one written: ask Kai to build it again"
            );
            self.record(id, request, &call, &format!("refused: {}", shown(&why)));
            return Reply::err(why);
        }
        let question = question(&built, &uses(&built.artifact.imports), &call);
        let menu = question
            .as_deref()
            .map_or(Err(termcore::menu::Refused::Unshowable), |question| {
                Menu::new(id, question, None, None, Origin::Added)
            })
            .and_then(|menu| menu.with_call(&call));
        let (Some(question), Ok(_)) = (question, menu) else {
            return Reply::err(format!(
                "Kai's app {name} cannot be asked about on the card: its call is too long"
            ));
        };
        self.host.audit(&format!("request {id}: {call} asks"));
        let checked = Checked::shown_only(&call);
        self.shown = Some(format!(
            "{}\nasked first: installing a new app always asks",
            crate::script::shown(id, &checked)
        ));
        self.pending = Some(Pending {
            id,
            request: request.to_string(),
            checked,
            asking: Vec::new(),
            created_ns: self.host.now_ns(),
            how: How {
                form: Form::Call,
                tier: Tier::Ask,
                consent: String::from("asked"),
            },
            call: None,
            offer: None,
            menu: Some(call.clone()),
            pkg: None,
            standing: None,
            model: None,
            app: None,
            install: Some(Installing {
                name: built.name.clone(),
                version: built.version.clone(),
                source: built.source.clone(),
                built_by: built.id,
            }),
        });
        Reply::menu(
            id,
            Menu::encode(id, Origin::Added, &question, "", "", &call),
        )
    }

    /// The answer to the install card: an Allow whose `ONCE` checks installs
    /// the build it was asked for, if that is still the one to install.
    pub(crate) fn answer_install(&mut self, pending: Pending, text: &str) -> Reply {
        let id = pending.id;
        let Some(asked) = pending.install.clone() else {
            return Reply::err("nothing to confirm");
        };
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        // The build may have changed while the card was up: another build, an
        // edit, or an install by an earlier card.
        let built = match self.installable(&asked.name) {
            Ok(built) => built,
            Err(why) => return return_refused(self, &pending, &why),
        };
        if (built.id, &built.version, &built.source)
            != (asked.built_by, &asked.version, &asked.source)
        {
            let why = format!(
                "Kai built {} {} after the card asked, so the card's build is not installed: ask again",
                built.name, built.version
            );
            return return_refused(self, &pending, &why);
        }
        self.host
            .audit(&format!("app_install {id}: allowed; installing it"));
        self.app_install(id, &pending, built)
    }

    /// Installs `built` through `pkg`'s `local`, and logs it with its undo.
    fn app_install(&mut self, id: u64, pending: &Pending, built: Built) -> Reply {
        let name = &built.name;
        let version = &built.version;
        let script = format!(
            "{}\n{}\n{}\n{}",
            pending.checked.script,
            source_line(name, &built.source),
            built.built.provenance,
            artifact_line(name, &built.artifact.sha256)
        );
        let how = How {
            consent: String::from("yes"),
            ..pending.how.clone()
        };
        // pkg signs and installs exactly the bytes the build recorded.
        let sha = Some(built.artifact.sha256.as_str());
        let answer = match self.host.pkg_local(name, &built.built.path, sha) {
            Ok(answer) => answer,
            Err(why) => {
                let why = shown(&why);
                let maybe = why.starts_with(NO_ANSWER);
                let outcome = format!("failed: local: {why}");
                self.push_install(id, &pending.request, how, script, &outcome, None);
                return Reply::err(match maybe {
                    true => format!(
                        "Kai's app {name} {version}: {why}: it may have installed it, and undo cannot switch that back"
                    ),
                    false => format!("Kai's app {name} {version} was not installed: {why}"),
                });
            }
        };
        let made = installed(&answer, name, version);
        let change = Change {
            action: String::from("app_install"),
            owner: String::from("pkg"),
            res: String::from("pkg-generation"),
            before: made.map_or_else(String::new, |(_, parent)| format!("{parent}")),
            after: made.map_or_else(String::new, |(n, _)| format!("{n}")),
            inverse: made.map(|(n, parent)| Inverse::Local {
                from: n,
                to: parent,
            }),
        };
        self.push_install(id, &pending.request, how, script, "ran", Some(change));
        self.remember(pending, "installed");
        match made {
            Some((n, _)) => Reply::ok(format!(
                "Installed Kai's app {name} {version} as generation {n}; it opens now. undo removes it, and its source stays in {APPS_DIR}/{name}/."
            )),
            None => Reply::ok(format!(
                "Installed Kai's app {name} {version}, but pkg's answer gives no undo: {}",
                shown(&answer)
            )),
        }
    }

    fn push_install(
        &mut self,
        id: u64,
        request: &str,
        how: How,
        script: String,
        outcome: &str,
        change: Option<Change>,
    ) {
        // Installed with or without an undo, it stands until an undo.
        let state = match outcome {
            "ran" => State::Done,
            _ => State::Nothing,
        };
        self.host.audit(&format!("app_install {id}: {outcome}"));
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: how.form,
            of: None,
            tier: Some(how.tier),
            pack: None,
            consent: how.consent,
            state,
            request: String::from(request),
            script,
            outcome: String::from(outcome),
            changes: change.into_iter().collect(),
        });
    }

    /// Whether `built` is installed: an `app_install` of its source stands in
    /// the log, not undone.
    fn app_installed(&self, built: &Built) -> bool {
        let line = source_line(&built.name, &built.source);
        self.log.entries().rev().any(|e| {
            e.state == State::Done
                && e.changes.iter().any(|c| c.action == "app_install")
                && e.script.lines().any(|l| l == line)
        })
    }

    /// `show uses`: every interface the last build of app `name` imports, as
    /// its artifact holds them; the card names them, or how many and this line.
    fn app_uses(&self, name: &str) -> Reply {
        match &self.app_built {
            Some(built) if built.name == name => {
                let imports = &built.artifact.imports;
                let list = match imports.is_empty() {
                    true => String::from("nothing"),
                    false => imports.join("\n"),
                };
                Reply::ok(format!(
                    "Kai's app {name} {} imports {} interface(s):\n{list}",
                    built.version,
                    imports.len()
                ))
            }
            _ => Reply::err(format!("Kai has not built an app called {name}")),
        }
    }

    /// `show code`: app `name`'s `main.c`, as it is now.
    fn app_code(&self, name: &str) -> Reply {
        match self.host.app_source_read(name) {
            Ok(Some(text)) => Reply::ok(format!("{APPS_DIR}/{name}/main.c:\n{text}")),
            Ok(None) => Reply::err(format!("{APPS_DIR}/{name}/main.c is not there")),
            Err(why) => Reply::err(format!("{APPS_DIR}/{name}/main.c: {}", shown(&why))),
        }
    }
}

/// Records `pending` as refused with `why`, and says so.
fn return_refused(shell: &mut Shell, pending: &Pending, why: &str) -> Reply {
    let outcome = format!("refused: {}", shown(why));
    shell.record(
        pending.id,
        &pending.request,
        &pending.checked.script,
        &outcome,
    );
    shell.remember(pending, why);
    Reply::err(String::from(why))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_lists_each_import_once_without_its_prefix_or_version() {
        let imports = [
            String::from("kai:app/log@0.1.0"),
            String::from("kai:ui/view@0.1.0"),
            String::from("kai:ui/view@0.1.0"),
        ];
        assert_eq!(uses(&imports), ["app/log", "ui/view"]);
        assert!(uses(&[]).is_empty());
    }
}
