//! Standing instructions (P3 plan task 11; the founder's decision of
//! 2026-10-27): a trigger, an action and an optional "until" event, such as
//! `on minimize(notes) -> app_suspend(notes) until restore(notes)`.
//!
//! This is the shell's half, run against any event source: the rule store as
//! `kai-standing 1` text, the check that a rule comes only from the user's own
//! line, and the engine that turns window events into firings with no storms.
//! The events and the actions come through [`Triggers`]: the desktop's window
//! events (G-core), comphost's suspend and resume (lane C) and the owners' check
//! that a live rule covers each firing (lane K, beside #149's `RULE_CHECK`)
//! plug in there as they land. The shell alone never decides that a firing is
//! allowed.

pub mod grammar;

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

pub const HEADER: &str = "kai-standing 1";
/// Most rules the store keeps.
pub const MAX_RULES: usize = 32;
/// A rule that fires more often than this in a minute is paused.
pub const MAX_PER_MINUTE: usize = 10;
const MINUTE_NS: u64 = 60_000_000_000;

/// What a trigger or an "until" waits for: `desktop`'s window events, idle
/// (no input for some minutes) and a time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Minimize,
    Restore,
    Focus,
    Close,
    /// No input for this many minutes.
    Idle(u16),
    /// This minute of the day, 0 to 1,439.
    Time(u16),
}

/// A trigger or an "until": an event kind and the app it names ("" for idle and
/// a time of day, which name none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct When {
    pub kind: Kind,
    pub app: String,
}

/// One event from the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub when: When,
    /// The rule whose firing caused it, if one did: a rule's own action never
    /// triggers it again.
    pub cause: Option<u64>,
}

/// What standing instructions need from the system. Until something implements
/// it, a rule is kept and listed but never fires.
pub trait Triggers {
    /// Delivers `when`'s events to the shell from now on; a time of day costs
    /// one timer.
    fn watch(&self, when: &When) -> Result<(), String>;
    /// Stops delivering them, when no rule waits for them any more.
    fn unwatch(&self, when: &When);
    /// Runs rule `rule`'s firing, `action` on `app`, or (`undo`) puts back what
    /// its last firing did, for `undo` or its "until". The action's owner checks
    /// that a live rule covers it, never the shell alone.
    fn run(&self, rule: u64, action: &str, app: &str, undo: bool) -> Result<(), String>;
}

/// Where a rule came from. Only the user's own line may make one (ADR 0002's
/// injection rule); text the AI read never does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Line,
    Data(&'static str),
}

/// One standing instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: u64,
    pub on: When,
    /// The action's row, and the app it acts on: the whole scope the user
    /// approved. A firing does exactly this and nothing more.
    pub action: &'static str,
    pub target: String,
    pub until: Option<When>,
    pub paused: bool,
    /// The rule node the Allow on its `rules_add` gave (P3 task 11): what its
    /// owner's RULE_CHECK finds, and what `rules_remove` revokes. A rule read
    /// back from disk has none until it is allowed again.
    pub grant: Option<u32>,
    /// Whether its last firing still stands, to be put back by `until`.
    armed: bool,
    /// When it fired in the last minute.
    fired: VecDeque<u64>,
}

impl Rule {
    /// `app_suspend("notes")`, the call each firing makes.
    pub fn call(&self) -> String {
        format!("{}(\"{}\")", self.action, self.target)
    }

    /// `on minimize(notes) -> app_suspend(notes) until restore(notes)`, as
    /// `rules` lists it.
    pub fn describe(&self) -> String {
        let mut text = format!(
            "on {} -> {}({})",
            when_text(&self.on),
            self.action,
            self.target
        );
        if let Some(until) = &self.until {
            let _ = write!(text, " until {}", when_text(until));
        }
        if self.paused {
            text.push_str(" (paused: it fired too often)");
        }
        text
    }
}

/// What the engine asks the shell to do for an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Firing {
    /// Run rule `rule`'s call, logged with the rule's id.
    Run { rule: u64, call: String },
    /// Its "until" came: put back rule `rule`'s last firing (`undo` of it).
    PutBack { rule: u64 },
    /// It fired more than [`MAX_PER_MINUTE`] times in a minute and is paused;
    /// the panel says so.
    Paused { rule: u64 },
}

/// Why a rule was not made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// It came from text the AI read, not the user's own line.
    FromData(&'static str),
    /// Its action is not a row a standing instruction may run: it must run at
    /// once with undo, so a firing never needs to ask when nobody is looking.
    NotRunTier(String),
    /// Its app or its "until" is not one a rule can name.
    Bad(&'static str),
    Full,
}

/// The standing instructions.
#[derive(Default)]
pub struct Standing {
    rules: Vec<Rule>,
    next: u64,
}

impl Standing {
    /// Makes a rule the user's Allow approved; its id. The store checks again
    /// what the ask card already showed: the line was the user's, and the
    /// action runs at once with undo.
    pub fn create(
        &mut self,
        origin: Origin,
        on: When,
        action: &str,
        target: &str,
        until: Option<When>,
    ) -> Result<u64, Refused> {
        let row = self.check(origin, &on, action, target, until.as_ref())?;
        self.next += 1;
        self.rules.push(Rule {
            id: self.next,
            on,
            action: row,
            target: target.to_string(),
            until,
            paused: false,
            grant: None,
            armed: false,
            fired: VecDeque::new(),
        });
        Ok(self.next)
    }

    /// Whether a rule could be made, before the ask card is shown: its row.
    pub fn check(
        &self,
        origin: Origin,
        on: &When,
        action: &str,
        target: &str,
        until: Option<&When>,
    ) -> Result<&'static str, Refused> {
        if let Origin::Data(source) = origin {
            return Err(Refused::FromData(source));
        }
        let row = runnable(action).ok_or_else(|| Refused::NotRunTier(action.to_string()))?;
        let names_app = |w: &When| w.app.is_empty() || is_app(&w.app);
        if !is_app(target) || !names_app(on) || !until.is_none_or(names_app) {
            return Err(Refused::Bad(
                "an app is named [a-z][a-z0-9-], at most 12 bytes",
            ));
        }
        if self.rules.len() == MAX_RULES {
            return Err(Refused::Full);
        }
        Ok(row)
    }

    /// A removed rule back under its own id (`undo` of `rules_remove`).
    pub fn put_back(&mut self, rule: Rule) -> Result<u64, Refused> {
        if self.rules.iter().any(|r| r.id == rule.id) {
            return Err(Refused::Bad("that rule is already there"));
        }
        if self.rules.len() == MAX_RULES {
            return Err(Refused::Full);
        }
        let id = rule.id;
        self.next = self.next.max(id);
        let at = self.rules.partition_point(|r| r.id < id);
        self.rules.insert(at, rule);
        Ok(id)
    }

    /// Every event some rule waits for, once each.
    pub fn watched(&self) -> Vec<When> {
        let mut out: Vec<When> = Vec::new();
        for when in self
            .rules
            .iter()
            .flat_map(|r| core::iter::once(&r.on).chain(&r.until))
        {
            if !out.contains(when) {
                out.push(when.clone());
            }
        }
        out
    }

    /// Keeps the rule node an Allow gave rule `id`.
    pub fn set_grant(&mut self, id: u64, grant: u32) {
        if let Some(rule) = self.rules.iter_mut().find(|r| r.id == id) {
            rule.grant = Some(grant);
        }
    }

    /// `rules_remove` of a standing instruction: it ends; what it did stays.
    pub fn remove(&mut self, id: u64) -> Option<Rule> {
        let at = self.rules.iter().position(|r| r.id == id)?;
        Some(self.rules.remove(at))
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// What `event`, at `now_ns`, asks for. Each rule fires at most once for one
    /// event, never for an event its own firing caused, never again while its
    /// last firing still stands, and pauses past [`MAX_PER_MINUTE`] firings.
    pub fn on_event(&mut self, event: &Event, now_ns: u64) -> Vec<Firing> {
        let mut out = Vec::new();
        for rule in &mut self.rules {
            if rule.paused || event.cause == Some(rule.id) {
                continue;
            }
            if rule.armed && rule.until.as_ref() == Some(&event.when) {
                rule.armed = false;
                out.push(Firing::PutBack { rule: rule.id });
                continue;
            }
            if rule.on != event.when || rule.armed {
                continue;
            }
            while rule
                .fired
                .front()
                .is_some_and(|&t| now_ns.saturating_sub(t) >= MINUTE_NS)
            {
                rule.fired.pop_front();
            }
            if rule.fired.len() >= MAX_PER_MINUTE {
                rule.paused = true;
                out.push(Firing::Paused { rule: rule.id });
                continue;
            }
            rule.fired.push_back(now_ns);
            // With no "until", nothing waits to put it back.
            rule.armed = rule.until.is_some();
            out.push(Firing::Run {
                rule: rule.id,
                call: rule.call(),
            });
        }
        out
    }

    /// The store as `kai-standing 1` text, one rule a line.
    pub fn text(&self) -> String {
        let mut out = String::from(HEADER);
        out.push('\n');
        for r in &self.rules {
            let until = r
                .until
                .as_ref()
                .map_or_else(|| String::from("-"), when_word);
            let _ = writeln!(
                out,
                "S {} on={} do={}:{} until={until} paused={}",
                r.id,
                when_word(&r.on),
                r.action,
                r.target,
                u8::from(r.paused)
            );
        }
        out
    }

    /// The store read back from [`Standing::text`]; `None` for text it could not
    /// have written. A rule read back is checked as if made again.
    pub fn parse(text: &str) -> Option<Self> {
        // Every line ends in one `\n`, the way `text` writes it.
        let mut lines = text.strip_suffix('\n')?.split('\n');
        if lines.next()? != HEADER {
            return None;
        }
        let mut store = Self::default();
        let mut last = 0;
        for line in lines {
            let mut f = line.strip_prefix("S ")?.split(' ');
            let id = canonical(f.next()?)?;
            let on = parse_when(f.next()?.strip_prefix("on=")?)?;
            let (action, target) = f.next()?.strip_prefix("do=")?.split_once(':')?;
            let until = match f.next()?.strip_prefix("until=")? {
                "-" => None,
                word => Some(parse_when(word)?),
            };
            let paused = match f.next()? {
                "paused=0" => false,
                "paused=1" => true,
                _ => return None,
            };
            if f.next().is_some() || store.rules.iter().any(|r| r.id == id) {
                return None;
            }
            // `create` numbers the rule `next + 1`: the id it was written with.
            store.next = id.checked_sub(1)?;
            store.create(Origin::Line, on, action, target, until).ok()?;
            store.rules.last_mut()?.paused = paused;
            last = last.max(id);
        }
        store.next = last;
        Some(store)
    }
}

impl Event {
    /// One line of the kernel's event box (EVENT_READ), as
    /// [`kabi::event::Event::parse`] reads it; `None` for any other line, such
    /// as the box's count of what it dropped.
    pub fn from_line(line: &str) -> Option<Self> {
        let event = kabi::event::Event::parse(line)?;
        let kind = match event.kind {
            kabi::event::Kind::Minimize => Kind::Minimize,
            kabi::event::Kind::Restore => Kind::Restore,
            kabi::event::Kind::Focus => Kind::Focus,
            kabi::event::Kind::Close => Kind::Close,
            kabi::event::Kind::Idle(minutes) => Kind::Idle(minutes),
            kabi::event::Kind::Time(minute) => Kind::Time(minute),
            // Desktop's wake: work for the shell, not a rule's event.
            kabi::event::Kind::Work => return None,
        };
        Some(Event {
            when: When {
                kind,
                app: event.app().to_string(),
            },
            cause: None,
        })
    }
}

/// The run-tier row a standing instruction may name, as `'static`.
fn runnable(action: &str) -> Option<&'static str> {
    agentabi::DESK_ROWS
        .iter()
        .chain(&agentabi::ROWS)
        .find(|row| row.name == action && !row.facts.read_only)
        .filter(|row| agentabi::derive(row) == agentabi::Tier::Run)
        .map(|row| row.name)
}

/// A package name (ADR 0011 §2): `[a-z][a-z0-9-]{1,11}`.
fn is_app(name: &str) -> bool {
    (2..=12).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn when_text(when: &When) -> String {
    match when.kind {
        Kind::Idle(minutes) => format!("idle({minutes} min)"),
        Kind::Time(minute) => format!("time({:02}:{:02})", minute / 60, minute % 60),
        kind => format!("{}({})", kind_word(kind), when.app),
    }
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Minimize => "minimize",
        Kind::Restore => "restore",
        Kind::Focus => "focus",
        Kind::Close => "close",
        Kind::Idle(_) => "idle",
        Kind::Time(_) => "time",
    }
}

/// `minimize:notes`, `idle:15`, `time:1320`.
fn when_word(when: &When) -> String {
    match when.kind {
        Kind::Idle(n) | Kind::Time(n) => format!("{}:{n}", kind_word(when.kind)),
        kind => format!("{}:{}", kind_word(kind), when.app),
    }
}

fn parse_when(word: &str) -> Option<When> {
    let (kind, rest) = word.split_once(':')?;
    let number = || canonical(rest).and_then(|n| u16::try_from(n).ok());
    let (kind, app) = match kind {
        "minimize" => (Kind::Minimize, rest),
        "restore" => (Kind::Restore, rest),
        "focus" => (Kind::Focus, rest),
        "close" => (Kind::Close, rest),
        "idle" => (Kind::Idle(number().filter(|&m| m > 0)?), ""),
        "time" => (Kind::Time(number().filter(|&m| m < 24 * 60)?), ""),
        _ => return None,
    };
    if !app.is_empty() && !is_app(app) {
        return None;
    }
    Some(When {
        kind,
        app: app.to_string(),
    })
}

/// A number as `text` writes it: digits only, no sign and no leading zero.
fn canonical(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    (digits && (text == "0" || !text.starts_with('0')))
        .then(|| text.parse().ok())
        .flatten()
}
