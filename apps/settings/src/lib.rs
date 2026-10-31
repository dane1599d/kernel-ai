//! The Settings app's model (P4 task 7): what each page shows and what the
//! user's choices ask for. It reads what the OS hands it (the apps' effect
//! rows, rules, profiles, the About facts and the three settings files) and
//! answers each choice with an [`Intent`]; the window carries the intent out as
//! a `kai:settings` request, which the shell logs and can undo. Nothing here
//! touches the OS itself.

#![no_std]

extern crate alloc;

pub mod access;
pub mod files;

use access::AppAccess;
use alloc::string::String;
use alloc::vec::Vec;
use files::{Appearance, Apps, Updates};

/// The pages, in the order the tabs show them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Appearance,
    /// What each app can do.
    Access,
    /// Standing rules and resource profiles.
    Rules,
    /// App nap's keep-running switches.
    Apps,
    /// The restart window.
    Updates,
    About,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::Appearance,
        Page::Access,
        Page::Rules,
        Page::Apps,
        Page::Updates,
        Page::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Access => "What apps can do",
            Page::Rules => "Rules and profiles",
            Page::Apps => "Keep running",
            Page::Updates => "Updates",
            Page::About => "About",
        }
    }
}

/// `system_about`'s answer (ADR 0009, "P1"): version, commit, licence, where
/// the source is, and the notices' summary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct About {
    pub version: String,
    pub commit: String,
    pub licence: String,
    pub source: String,
    pub notices: String,
}

/// What `rules` lists and `rules_remove` takes: a standing rule by its number,
/// or a standing instruction as `S<n>` (P3 task 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleRef {
    Rule(u32),
    Instruction(u32),
}

impl RuleRef {
    /// The word `rules_remove` takes: `3` or `S3`.
    pub fn word(self) -> String {
        match self {
            RuleRef::Rule(n) => alloc::format!("{n}"),
            RuleRef::Instruction(n) => alloc::format!("S{n}"),
        }
    }

    /// `3` or `S3`, as `rules` lists them; `None` for anything else.
    pub fn parse(word: &str) -> Option<RuleRef> {
        let number = |digits: &str| {
            let ok = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
            ok.then(|| digits.parse().ok()).flatten()
        };
        match word.strip_prefix('S') {
            Some(rest) => number(rest).map(RuleRef::Instruction),
            None => number(word).map(RuleRef::Rule),
        }
    }
}

/// One line of `rules`: what it is and its words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleLine {
    pub id: RuleRef,
    pub text: String,
}

/// What Settings asks the OS to do for a choice. Each is one of the agent's
/// rows, so it is logged and undoable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `appearance_set`.
    Appearance(Appearance),
    /// Withholds one effect row from an app (`grants-revoke <app> <row>`).
    Revoke { app: String, row: String },
    /// Gives a withheld row back (`grants-restore <app> <row>`).
    Restore { app: String, row: String },
    /// `rules_remove`, with [`RuleRef::word`].
    RemoveRule(RuleRef),
    /// `profile_set`.
    UseProfile(String),
    /// The keep-running switch for one app.
    KeepRunning { app: String, on: bool },
    /// Writes `updates.cbor`.
    RestartWindow(Option<(u16, u16)>),
}

/// Everything the pages show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub appearance: Appearance,
    pub access: Vec<AppAccess>,
    pub rules: Vec<RuleLine>,
    /// Profile names, and the one in use.
    pub profiles: Vec<String>,
    pub profile: Option<String>,
    /// Every app that can nap, by name; [`Settings::apps`] says which stay.
    pub nappable: Vec<String>,
    pub apps: Apps,
    pub updates: Updates,
    pub about: About,
}

impl Settings {
    /// The theme after `name`, wrapping: the Appearance page's "Theme" button.
    pub fn next_theme(&self) -> Intent {
        let themes = look::THEMES;
        let at = themes
            .iter()
            .position(|p| p.name == self.appearance.theme)
            .map_or(0, |i| (i + 1) % themes.len());
        Intent::Appearance(Appearance {
            theme: String::from(themes[at].name),
            ..self.appearance.clone()
        })
    }

    /// The row `row` of `app`, if the shell listed it.
    fn row(&self, app: &str, row: &str) -> Option<&access::Access> {
        let app = self.access.iter().find(|a| a.app == app)?;
        app.access.iter().find(|x| x.row == row)
    }

    /// Withholding `row` from `app`, if it is listed and allowed now.
    pub fn revoke(&self, app: &str, row: &str) -> Option<Intent> {
        self.row(app, row)?.allowed.then(|| Intent::Revoke {
            app: String::from(app),
            row: String::from(row),
        })
    }

    /// Giving `row` back to `app`, if it is listed and withheld now.
    pub fn restore(&self, app: &str, row: &str) -> Option<Intent> {
        (!self.row(app, row)?.allowed).then(|| Intent::Restore {
            app: String::from(app),
            row: String::from(row),
        })
    }

    /// Flipping `app`'s keep-running switch, if it is an app that naps.
    pub fn keep_running(&self, app: &str, on: bool) -> Option<Intent> {
        let known = self.nappable.iter().any(|a| a == app);
        (known && self.apps.keeps(app) != on).then(|| Intent::KeepRunning {
            app: String::from(app),
            on,
        })
    }

    /// Setting the restart window; `None` turns it off. Ends that are not
    /// minutes of a day, or the same minute, are refused.
    pub fn restart_window(&self, window: Option<(u16, u16)>) -> Option<Intent> {
        let window = match window {
            Some((start, end)) => Some(files::window(start, end)?),
            None => None,
        };
        (window != self.updates.restart_window).then_some(Intent::RestartWindow(window))
    }

    /// Records that `app`'s `row` is allowed or withheld now; a row that is not
    /// listed stays unlisted.
    fn allow(&mut self, app: &str, row: &str, allowed: bool) {
        let rows = self.access.iter_mut().filter(|a| a.app == app);
        for x in rows
            .flat_map(|a| a.access.iter_mut())
            .filter(|x| x.row == row)
        {
            x.allowed = allowed;
        }
    }

    /// Applies an intent the OS carried out, so the pages show it.
    pub fn applied(&mut self, intent: &Intent) {
        match intent {
            Intent::Appearance(a) => self.appearance = a.clone(),
            Intent::Revoke { app, row } => self.allow(app, row, false),
            Intent::Restore { app, row } => self.allow(app, row, true),
            Intent::RemoveRule(id) => self.rules.retain(|r| r.id != *id),
            Intent::UseProfile(name) => self.profile = Some(name.clone()),
            Intent::KeepRunning { app, on } => {
                self.apps.set(app, *on);
            }
            Intent::RestartWindow(w) => self.updates.restart_window = *w,
        }
    }
}
