//! The Rhai shell, kept free of system calls so it can be tested on the host. The
//! ring-3 `shell` program supplies a [`Host`] and feeds it one line at a time.
//!
//! Like TempleOS's prompt, every line is code: variables and functions persist, and
//! the value of the last expression is echoed. Plain commands (`mem`, `policy rr`,
//! `ask why is the sky blue`) are recognised from their first word and run directly.
//! A line starting with `?`, or plain words that do not run as Rhai, go to the LLM,
//! which answers in words or with a script. A script is checked, then its tier
//! (`tiers`) decides: one whose every function can be undone runs at once and the
//! reply says what happened; one that cannot asks one short question first (see
//! `actions`). `undo` reverts the last run, and `show` or `why` shows its script.
//!
//! Every run is bounded: by time (checked every [`CHECK_EVERY`] operations), by an
//! operation count, by heap use, and by Rhai's own size and depth limits. A run that
//! hits a bound returns an error and leaves the shell running.
//!
//! An entry with a brace, bracket or parenthesis still open is kept, and the reply
//! is `more`: the next line continues it (the terminal shows `..> `).

#![no_std]

extern crate alloc;

mod abilities;
mod actions;
pub mod actlog;
mod agentconsent;
mod agentcore;
pub mod agentstep;
pub mod analyse;
pub mod apart;
pub mod appbuild;
mod appearance;
mod appgrants;
mod appinstall;
pub mod appjob;
pub mod appline;
pub mod apps;
mod brackets;
pub mod ccbuild;
mod centry;
mod clarify;
mod clock;
mod cmode;
mod commands;
pub mod data;
mod deskask;
mod deskcard;
pub mod door;
mod effects;
mod engine;
mod eval;
mod goalqueue;
mod goalrows;
mod goalscope;
mod goaltoggle;
pub mod grants;
mod guard;
mod host;
mod instructions;
mod landing;
pub mod ledger;
mod models;
mod netask;
pub mod netgrant;
pub mod outbox;
mod sound;
mod startup;
pub use models::MODELS_APP;
pub use netask::Connect as NetConnect;
mod packages;
mod paging;
mod panel;
mod pkg;
mod power;
mod profiles;
mod request;
mod rules;
pub mod script;
mod sending;
mod sendrow;
mod settings;
pub use settings::{MAX_ANSWER as SETTINGS_MAX_ANSWER, SETTINGS_APP};
mod show;
mod sitewords;
pub mod standing;
pub mod state;
pub mod targets;
pub mod tiers;
mod tune;
pub mod ui;
mod undo;
mod undoing;
pub mod waiting;

use actions::Pending;
pub use agentcore::Outcome;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use centry::CallValue;
/// Repairs a Kai C program at `c>` gets after its first answer (P4 task 14).
pub use cmode::C_REPAIRS;
pub use effects::EffectDone;
pub use engine::Exec;
use engine::Line;
pub use guard::placeholder_target;
pub use host::{Consent, FsChange, FsRead, FsRefused, FsUndone, Host, NO_FILES, NO_NET, Proof};
pub use ledger::LogFailed;
pub use panel::PanelResult;
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};
use tiers::Tier;

pub use commands::{COMMANDS, split_command};

/// What the state files read once at start are called in what is audited.
pub(crate) const RULES: &str = "rules";
pub(crate) const STANDING: &str = "standing instructions";
pub(crate) const KEEP: &str = "keep-running list";

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Longest a line may compute, not counting time spent waiting in `ask`.
    pub line_budget_ms: u64,
    /// Heap a line may add before it is stopped.
    pub heap_budget: u64,
    /// Heap an LLM-written script may add, in its trial run and its real one. Far
    /// smaller than `heap_budget`: one operation can copy a value several times over
    /// (a clone, or text formatted with escapes), the trial runs before anyone has
    /// read the script, and the shell has 16 MiB in all.
    pub script_heap_budget: u64,
    /// Most bytes of printed output kept per line.
    pub max_output: usize,
    /// Backstop if the clock check never fires. Far above what the time budget
    /// allows on fast hardware (KVM runs over 10 million operations a second), so
    /// the time limit is what normally stops a line.
    pub max_operations: u64,
    /// How typed lines and scripts are run.
    pub exec: Exec,
}

pub const DEFAULT_CONFIG: Config = Config {
    line_budget_ms: 2000,
    heap_budget: 8 * 1024 * 1024,
    script_heap_budget: 1024 * 1024,
    max_output: 6 * 1024,
    max_operations: 2_000_000_000,
    exec: Exec::Ast,
};

/// Longest entry the shell gathers over continuation lines (ADR 0012, "Modes").
pub const MAX_ENTRY: usize = 4 * 1024;
/// What the terminal sends when Ctrl-C clears a continuation line: the entry so far
/// is dropped. A typed line cannot hold a control character.
pub const CANCEL_ENTRY: &str = "\u{3}";

/// How often the time, interrupt and heap checks run, in operations.
pub const CHECK_EVERY: u64 = 1024;
/// Time the llm service gets for one script request; two rounds (an answer and one
/// repair) must fit in the terminal's wait for a line (310 s since the M10 demo).
pub const SCRIPT_ROUND_MS: u64 = 70_000;
/// First words that mark a line as Rhai even when it does not parse.
const RHAI_WORDS: &[&str] = &[
    "let", "const", "fn", "if", "else", "for", "while", "loop", "return", "print", "throw", "try",
    "import",
];

/// The shell's answer to one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub ok: bool,
    pub text: String,
    /// Set when the reply asks the user about script `id`.
    pub confirm: Option<u64>,
    /// Set when a brace, bracket or parenthesis is still open: the next line
    /// continues this entry, and the terminal shows `..> `.
    pub more: bool,
}

impl Reply {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
            confirm: None,
            more: false,
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
            confirm: None,
            more: false,
        }
    }

    fn more() -> Self {
        Self {
            more: true,
            ..Self::ok("")
        }
    }

    /// The Allow / Deny menu for call `id`, drawn by the terminal: `text` is
    /// `termcore::menu::Menu::encode`'s.
    fn menu(id: u64, text: String) -> Self {
        Self {
            ok: true,
            text,
            confirm: Some(id),
            more: false,
        }
    }

    /// `ok <text>`, `err <text>`, `confirm <id> <text>`, `menu <id> ...` or `more`,
    /// as sent to the terminal.
    pub fn to_wire(&self) -> String {
        match self.confirm {
            _ if self.more => String::from("more"),
            Some(_)
                if self.text.starts_with("menu ")
                    || self.text.starts_with(termcore::menu::SHOWN) =>
            {
                self.text.clone()
            }
            Some(id) => format!("confirm {id} {}", self.text),
            None => format!("{} {}", if self.ok { "ok" } else { "err" }, self.text),
        }
    }
}

pub struct Shell {
    engine: Engine,
    scope: Scope<'static>,
    /// Every function defined so far, and no statements.
    functions: AST,
    line: Rc<Line>,
    host: Rc<dyn Host>,
    config: Config,
    pending: Option<Pending>,
    next_id: u64,
    /// The action log: every request, what it changed, and its undo.
    log: actlog::ActLog,
    tuner: tune::Tuner,
    /// Each script API function's tier.
    tiers: &'static [(&'static str, Tier)],
    /// The entry an `undo` waits on a yes for, because something changed it since.
    undo_asking: Option<u64>,
    /// Standing rules the menu's "Always allow" wrote.
    rules: rules::Rules,
    /// Standing instructions (P3 task 11), and the ones `rules_remove` ended,
    /// which `undo` can put back until the shell restarts.
    standing: standing::Standing,
    instructions_removed: Vec<standing::Rule>,
    /// Resource profiles (P3 task 12), in RAM until `/system/settings`.
    profiles: profiles::Profiles,
    /// The apps kept running, never napped (`/system/state/keep`).
    keep: Vec<String>,
    /// Saved kept apps the supervisor could not reach yet at boot (an app not
    /// serving yet), and when to try them again and when to give up.
    keep_waiting: settings::KeepWaiting,
    /// Each standing instruction's last profile switch, which its "until" puts back.
    fired_profiles: Vec<(u64, u64)>,
    /// What `app_build` built last and has not installed ("Kai builds apps").
    app_built: Option<appbuild::Built>,
    /// Each app's `main.c` this session wrote, by its SHA-256: overwritten
    /// again only while it is still that.
    app_written: Vec<(String, String)>,
    /// The `app_build` going on between lines, if one is.
    app_job: Option<appjob::Job>,
    /// What builds in the background came to, not yet shown.
    notices: Vec<String>,
    /// File changes that did not land, as the line's reply says them.
    landing: landing::Landing,
    /// The app whose install card waits to open by itself, once a build in
    /// the background built it ([`Shell::take_card`]).
    card: Option<String>,
    /// Proposals from desktop lines (the panel, the Terminal window) waiting
    /// for the trusted card (`deskask`).
    desk_asks: Vec<deskask::Waiting>,
    /// The question on the card that came from a desktop line, and where
    /// that line was typed.
    card_for_desk: Option<(u64, deskask::Asker)>,
    /// The card's question whose agent was retired while it was up: its
    /// answer runs nothing and says so (`agentconsent::RETIRED`).
    retired_card: Option<u64>,
    /// What a panel line's card came to, for the panel.
    card_result: Option<(bool, String)>,
    /// The send a panel line's card allowed and the outbox holds, for the
    /// panel to show as that line's ([`Shell::take_card_held`]).
    card_held: Option<u64>,
    /// What windows' lines' cards came to, and their asks for it (`deskcard`).
    card_answers: deskcard::Answers,
    /// Apps' network asks waiting for the terminal's card (P5 task 13).
    net_cards: Vec<netask::NetAsking>,
    /// Network grants revoked, kept for their undo.
    net_revoked: Vec<kaigrants::Grant>,
    /// The highest network grant id this run gave or saw: new grants are
    /// numbered above it, so an id is never reused while the shell runs.
    net_high: u32,
    /// Each network card the person answered Deny: refused with no card for
    /// `netask::DENY_HOLDS_NS`.
    net_denied: Vec<netask::Denied>,
    /// Installed software: the image's repository and its generations.
    pkgs: pkg::Store,
    /// Whether the `sudo` note was said already.
    said_no_sudo: bool,
    /// The words that name a site for the agent's network rows: the curated host
    /// table, and each installed app's word for its own hosts (P5 task 11).
    hosts: nethosts::Table,
    /// What the agent's reading of app grants keeps between passes
    /// ([`appgrants`]).
    grant_memo: appgrants::Memo,
    /// The agents the shell knows (ADR 0013 §1): who a badged route's step is.
    roster: agentid::Roster,
    /// What the card came to for agents' asked steps, until each waits for it.
    agents_told: Vec<agentstep::Told>,
    /// The goals the person set and the queue to `goals` (G2).
    goal_book: goalrows::GoalBook,
    /// The admission of the agent whose step is running: no rule of the
    /// person's covers it (`agentconsent`).
    acting: Option<agentid::Admission>,
    /// The first entry id of the step `acting` is for: older entries (a
    /// question that lapses during the step) are not its.
    acting_from: u64,
    /// Agents' open questions, and the admission each was asked under.
    agent_asks: agentid::Asks,
    agent_admissions: Vec<(u64, agentid::Admission)>,
    /// The agent's question the person's yes was just spent on.
    agent_spent: Option<u64>,
    /// The last script as `show` prints it.
    shown: Option<String>,
    /// What the script run by the last line printed.
    last_output: Option<String>,
    /// What the last finished script showed, and the shell's line after it: a
    /// model turn whose reply printed Content is remembered without its text
    /// (#831).
    showed: agentcore::Showed,
    /// An entry with a brace, bracket or parenthesis still open, waiting for its
    /// next line.
    partial: Option<String>,
    /// A request the model asked to clarify: the next line is joined to it.
    clarifying: Option<String>,
    /// The terminals of apps that run shell lines, by app (`appline`): each has an
    /// entry, question and proposal of its own.
    fronts: alloc::collections::BTreeMap<String, panel::Terminal>,
    /// Since the last panel line: whether the line went to the model, whether
    /// it answered in words, and code's result line for what ran.
    asked_model: bool,
    worded: bool,
    done_line: Option<String>,
    /// The log entry the last panel line made, if it made one: what the
    /// panel's undo link undoes ([`Shell::take_panel_entry`]).
    panel_entry: Option<u64>,
    /// What the AI said to the last panel line, for the desk's history
    /// ([`Shell::take_panel_said`]): the model's words or a model script's
    /// prints, never the panel's own state line.
    panel_said: Option<String>,
    /// The panel's own clarifying question, kept for its next line as an
    /// app's terminal keeps its own: the person answers in the bar.
    panel_clarifying: Option<String>,
    /// A question code asked a panel line (which target), for the desk's
    /// history as code's note ([`Shell::take_panel_note`]): it can carry a
    /// name the model chose, so it is never the bar's.
    panel_note: Option<String>,
    /// `new` ran since the panel's line began: the terminal's question,
    /// set aside meanwhile, is forgotten too.
    forgot: bool,
    /// The last clarifying question was the model's (`clarify`), not code's
    /// (`guard::which`, a target to pick): only the model's words go to the
    /// desk's history as Kai's.
    clarified_by_model: bool,
    /// The `pack_id` of the pack the model is given, logged with each model turn
    /// (ADR 0010 §7): the same one llm reports for its `act` slot.
    pack_id: u64,
    /// The log text last written to `/system/log`, so an unchanged log is not
    /// written again.
    saved_log: Option<String>,
    /// The state files read once at start that could not be read: never
    /// written over this run ([`Shell::read_at_start`]).
    unread: Vec<&'static str>,
    /// The ONCE of the menu answer now running, kept for the call's owner to
    /// check: the shell does not spend it (P1 task 14).
    owner_once: Option<u64>,
    /// The source of the data the call now running was decided after (a
    /// view's second round), for consent (ADR 0002 D6).
    steered: Option<&'static str>,
    /// The line now running is the person's own `open <app>` (`open_line`),
    /// made by code with no model: its calls are the person's (`Checked::person`).
    person_line: bool,
    /// The `c>` entry now running, and its calls' values (P4 task 12).
    c_entry: Option<centry::CEntry>,
    last_value: Option<rhai::Dynamic>,
    value_rets: Vec<(u64, agentabi::RetType)>,
    values: Vec<(u64, centry::CallValue)>,
    /// The `c>` prompt's state.
    c: cmode::CMode,
    /// Sends allowed and held before they go, with `undo` live (`outbox`).
    outbox: outbox::Outbox,
    /// What sends came to since the panel last looked.
    sends_ended: Vec<outbox::Ended>,
}

impl Shell {
    /// The State the next request carries, `#{...}`, or `None` when the OS knows
    /// nothing to show (ADR 0010 §5).
    pub fn state_line(&self) -> Option<String> {
        self.host.state().snapshot()
    }
}

impl Shell {
    pub fn new(host: Rc<dyn Host>, config: Config) -> Self {
        Self::with_tiers(host, config, tiers::TIERS)
    }

    /// A shell whose scripts are tiered by `tiers` instead of [`tiers::TIERS`].
    pub fn with_tiers(
        host: Rc<dyn Host>,
        config: Config,
        tiers: &'static [(&'static str, Tier)],
    ) -> Self {
        let line = Line::new();
        let mut engine = engine::bounded(&line, &host, &config);
        commands::register(&mut engine, &host, &line, config.line_budget_ms);
        Self {
            engine,
            scope: Scope::new(),
            functions: AST::empty(),
            line,
            host,
            config,
            pending: None,
            next_id: 1,
            log: actlog::ActLog::default(),
            tuner: tune::Tuner::default(),
            tiers,
            undo_asking: None,
            rules: rules::Rules::default(),
            standing: standing::Standing::default(),
            instructions_removed: Vec::new(),
            profiles: profiles::Profiles::default(),
            keep: Vec::new(),
            keep_waiting: settings::KeepWaiting::default(),
            fired_profiles: Vec::new(),
            app_built: None,
            app_written: Vec::new(),
            app_job: None,
            notices: Vec::new(),
            landing: landing::Landing::default(),
            card: None,
            desk_asks: Vec::new(),
            card_for_desk: None,
            retired_card: None,
            card_result: None,
            card_held: None,
            card_answers: deskcard::Answers::default(),
            net_cards: Vec::new(),
            net_revoked: Vec::new(),
            net_high: 0,
            net_denied: Vec::new(),
            pkgs: pkg::Store::default(),
            said_no_sudo: false,
            hosts: nethosts::Table::curated(),
            roster: agentid::Roster::new(),
            agents_told: Vec::new(),
            goal_book: goalrows::GoalBook::default(),
            acting: None,
            acting_from: 0,
            agent_asks: agentid::Asks::new(),
            agent_admissions: Vec::new(),
            agent_spent: None,
            grant_memo: appgrants::Memo::default(),
            shown: None,
            last_output: None,
            showed: agentcore::Showed::default(),
            partial: None,
            clarifying: None,
            fronts: alloc::collections::BTreeMap::new(),
            asked_model: false,
            worded: false,
            done_line: None,
            panel_entry: None,
            panel_said: None,
            panel_clarifying: None,
            panel_note: None,
            forgot: false,
            clarified_by_model: false,
            pack_id: llmproto::pack::act_pack_id(),
            saved_log: None,
            unread: Vec::new(),
            owner_once: None,
            steered: None,
            person_line: false,
            c_entry: None,
            last_value: None,
            value_rets: Vec::new(),
            values: Vec::new(),
            c: cmode::CMode::default(),
            outbox: outbox::Outbox::default(),
            sends_ended: Vec::new(),
        }
        .with_saved_log()
        .with_saved_rules()
        .with_saved_standing()
        .with_saved_keep()
        .with_saved_withheld()
    }

    /// What the script the last line ran printed, then its error if it failed;
    /// `None` if the line ran no script.
    pub fn last_output(&self) -> Option<&str> {
        self.last_output.as_deref()
    }

    /// Runs one line and returns what to show; the log goes to disk after it.
    pub fn eval(&mut self, input: &str) -> Reply {
        if let Some(reply) = self.status_line(input) {
            return reply;
        }
        self.landing_begins();
        let reply = self.eval_line(input);
        self.save_log();
        self.say_not_landed(reply)
    }

    /// [`Shell::eval`] for the terminal's own line: a build's notice goes
    /// ahead of a plain reply, and a question keeps it for the next. The
    /// panel's and apps' lines go through `eval`, so their result stays
    /// theirs (C on #621).
    pub fn eval_console(&mut self, input: &str) -> Reply {
        // The terminal asks for the card the shell said waits: never a typed
        // line, and never the panel's or an app's.
        if input == termcore::menu::TAKE_CARD {
            let reply = self.take_card();
            self.save_log();
            return reply;
        }
        if let Some(reply) = self.status_line(input) {
            return reply;
        }
        let answering = input.strip_prefix("answer ").and_then(|rest| {
            let mut words = rest.split(' ');
            let id = words.next()?.parse::<u64>().ok()?;
            Some((id, words.next().unwrap_or("")))
        });
        if answering.is_some_and(|(id, _)| self.on_card_for_desk(id)) {
            self.done_line = None;
        }
        // Read before the answer runs: its question's age decides a lapse.
        let lapsed = answering.is_some_and(|(id, _)| self.card_lapsed(id));
        // An agent's question takes the person's answer here, at the
        // terminal, or not at all (`agentconsent`).
        if let Some((id, word)) = answering
            && !lapsed
            && let Some(refused) = self.agent_answer(id, word, Shell::console_mark())
        {
            self.save_log();
            return refused;
        }
        let mut reply = self.eval(input);
        self.desk_answered(answering, &reply, lapsed);
        if reply.confirm.is_none() && !reply.more && !self.notices.is_empty() {
            let mut text = self.take_notices().join("\n");
            if !reply.text.is_empty() {
                text.push('\n');
                text.push_str(&reply.text);
            }
            reply.text = text;
        }
        reply
    }

    /// A line that only reports state: answered before any other line's
    /// handling, so it never discards a pending card, a proposal or an undo
    /// that waits, never takes queued notices, and writes no log. `net-grant`'s
    /// case polls `net denied` while a card waits (#818's batch failures).
    fn status_line(&mut self, input: &str) -> Option<Reply> {
        if self.partial.is_some() || self.scope.contains("net") {
            return None;
        }
        match input.trim() {
            "net denied" => Some(self.net_denied_line()),
            _ => None,
        }
    }

    /// Every `let` adds a scope entry, even for a name that exists: keep only the
    /// newest of each, so the variable limit counts names, not assignments.
    fn compact_scope(&mut self) {
        let entries: Vec<(String, bool, Dynamic)> = self
            .scope
            .iter()
            .map(|(name, constant, value)| (name.to_string(), constant, value))
            .collect();
        if entries.len() < 2 {
            return;
        }
        let mut scope = Scope::new();
        for (index, (name, constant, value)) in entries.iter().enumerate() {
            if entries[index + 1..].iter().any(|(later, ..)| later == name) {
                continue;
            }
            if *constant {
                scope.push_constant_dynamic(name.clone(), value.clone());
            } else {
                scope.push_dynamic(name.clone(), value.clone());
            }
        }
        self.scope = scope;
    }

    /// `new`: llm forgets the conversation. The action log and `undo` stay: what
    /// was done can still be undone.
    fn forget(&mut self) -> Reply {
        if let Err(why) = self.host.forget() {
            return Reply::err(format!("The conversation was not cleared: {why}"));
        }
        self.clarifying = None;
        // The panel's question goes with the conversation.
        self.panel_clarifying = None;
        self.forgot = true;
        Reply::ok("A new conversation: earlier turns are forgotten; `log` and `undo` still work")
    }

    /// How many variables and functions the shell holds.
    pub fn sizes(&self) -> (usize, usize) {
        (self.scope.len(), self.functions.iter_functions().count())
    }
}

/// A line that does not run as Rhai is sent to the LLM if it reads like words: mostly
/// letters and no code punctuation. One starting with a Rhai keyword is Rhai with a
/// typo unless it is a sentence: "import the model on the attached disk" is words,
/// "let x be 5" and "print hello world" are code.
pub fn reads_like_words(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() || (RHAI_WORDS.contains(&words[0]) && !is_sentence(&words)) {
        return false;
    }
    if line.chars().any(|c| "(){}[];=\"`#$".contains(c)) {
        return false;
    }
    let letters = line.chars().filter(|c| c.is_alphabetic()).count();
    let others = line
        .chars()
        .filter(|c| !c.is_alphabetic() && !c.is_whitespace())
        .count();
    letters >= 4 * others
}

/// The most characters of a left question the note quotes.
const LEFT_NOTE_CHARS: usize = 40;

/// The fewest words a line led by a Rhai keyword needs to read as a sentence.
const SENTENCE_WORDS: usize = 4;

/// Whether `words` are a sentence: at least [`SENTENCE_WORDS`] of them, each
/// letters with at most an inner `'` or `-` and a trailing `,` `.` `?` or `!`.
/// A digit or an operator anywhere makes it code.
fn is_sentence(words: &[&str]) -> bool {
    words.len() >= SENTENCE_WORDS
        && words.iter().all(|word| {
            let word = word.trim_end_matches([',', '.', '?', '!']);
            !word.is_empty()
                && word.starts_with(char::is_alphabetic)
                && word
                    .chars()
                    .all(|c| c.is_alphabetic() || c == '\'' || c == '-')
        })
}

/// A name that reads as a word, not as code: two or more letters and nothing else.
fn is_word(name: &str) -> bool {
    name.chars().count() > 1 && name.chars().all(char::is_alphabetic)
}

/// A Rhai error from a message.
fn runtime_error(message: impl Into<String>) -> alloc::boxed::Box<EvalAltResult> {
    let message: String = message.into();
    message.into()
}

#[cfg(test)]
mod tests;
