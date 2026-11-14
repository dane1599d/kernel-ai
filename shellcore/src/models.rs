//! The `model_*` rows (ADR 0008 §11; P4 task 10), which `updated` carries out
//! through the shell's route to it ([`crate::Host::models`]):
//!
//! - `model_list` answers what `updated` lists, `kai-models 1`, in words;
//! - `model_use` names a stored model the signed role takes for the next
//!   restart, on trial. It runs when the line names the model, and is logged
//!   with `Inverse::Model`, so `undo` puts the model before back;
//! - `model_fetch` imports the attached disk's model (`disk`), or downloads a
//!   signed one (`net`, by its id or the recommended one), held to the signed
//!   role. It always asks once: gigabytes of new bytes, and no undo yet. The
//!   download asks with its size and host, as `models download <id>` does.
//!
//! What `updated` answers is the OS's own state, so it is the reply as it is.
//!
//! A person types the same rows, before Rhai and with no model running, as a
//! package line is typed ([`parse_line`]): `models` lists, `models use <id>`
//! switches and is logged with its undo, and `models import` asks once, as the
//! AI's `model_fetch` does. On a fresh image no model runs yet, so this is how
//! the first one gets in.
//!
//! The Models window switches through the same row (#556): a person's key or
//! click on Use comes from `desktop` as `use <sha256>` ([`Shell::models_change`]),
//! held as a Settings change is, and is logged with the window's click as its
//! consent and the same undo. An import is not taken from the window: it asks,
//! and only a line on the trusted path can answer.

use crate::actions::{How, Pending};
use crate::actlog::{Change, Entry, Form, State};
use crate::script::Checked;
use crate::tiers::Tier;
use crate::undo::{Inverse, Refused};
use crate::{Reply, Shell};
use agentabi::consent::{Decision, Why};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use comp_models::{Models, Sha256, hex, size};
use modelchan::fetch::{self, Fit};
use modelchan::{listing, offered};

/// The one app whose switches the shell takes: the model manager's window, by
/// its plan line's name, the one the image grants `kai:models`.
pub const MODELS_APP: &str = modelchan::asker::WINDOW;
/// The consent a switch from the Models window is logged with: a person's key
/// or click in it.
const CLICK: &str = "user-click:models";

/// What waits on an asked model row's answer.
#[derive(Clone, Debug)]
pub(crate) enum Waiting {
    Use {
        to: Sha256,
        back: Option<Sha256>,
    },
    Fetch,
    /// A download of the signed target `id`, asked as `subject`.
    Download {
        id: String,
        subject: String,
    },
}

impl Shell {
    /// Answers a switch from app `app` (#556): `use <sha256>`, made by a
    /// person's key or click (`user`), run as Kai's `model_use` row with that
    /// consent and logged with its undo. The answer's text, or why not. The
    /// app and `user` are checked here again, whatever `desktop` and
    /// `comphost` did.
    pub fn models_change(
        &mut self,
        app: &str,
        request: &str,
        user: bool,
    ) -> Result<String, String> {
        if app != MODELS_APP {
            return Err(String::from("only the Models window switches the model"));
        }
        let Some(sha) = request
            .strip_prefix(kabi::model::SWITCH)
            .filter(|s| kabi::model::is_sha256_hex(s))
        else {
            return Err(String::from("not a model switch"));
        };
        if !user {
            return Err(String::from("a switch needs a key or a click"));
        }
        let id = self.next_id;
        self.next_id += 1;
        let decision = Decision {
            tier: Tier::Run,
            why: Why::Asked,
        };
        let reply = self.model_use(id, &format!("models: {request}"), sha, decision, CLICK);
        self.save_log();
        match reply.ok {
            true => Ok(reply.text),
            false => Err(reply.text),
        }
    }
}

impl Shell {
    fn models_now(&self) -> Result<Models, String> {
        let text = self.host.models("models")?;
        if let Some(why) = text.strip_prefix("err ") {
            return Err(String::from(why));
        }
        listing::parse(&text).map_err(|why| format!("updated's list: {why}"))
    }

    /// A `model_*` row the AI called, after consent.
    pub(crate) fn model_call(
        &mut self,
        id: u64,
        request: &str,
        row: &str,
        values: &[(&'static str, String)],
        decision: Decision,
    ) -> Reply {
        let name = values
            .iter()
            .find(|(n, _)| *n == "name")
            .map(|(_, v)| v.as_str())
            .unwrap_or_default();
        match row {
            "model_list" => match self.models_now() {
                Ok(models) => Reply::ok(listed(&models)),
                Err(why) => Reply::err(why),
            },
            "model_use" => self.model_use(id, request, name, decision, "asked"),
            "model_fetch" if value(values, "source") == Some("net") => {
                let name = match name {
                    "" => match self.offer_to_download() {
                        Ok(name) => name,
                        Err(why) => return Reply::err(why),
                    },
                    name => String::from(name),
                };
                self.model_download(id, request, &name)
            }
            "model_fetch" => {
                let call = canonical("model_fetch", "disk");
                let why = match decision.why {
                    Why::TakenAs | Why::Grounded | Why::Read => Why::NoUndo,
                    why => why,
                };
                self.model_ask(id, request, &call, Waiting::Fetch, why, None)
            }
            other => Reply::err(format!("{other} is typed at the prompt for now")),
        }
    }

    /// The model a `model_fetch("net")` with no name downloads: the one the
    /// signed role recommends, if this machine may download it now, else the
    /// first it may. With none, why, as `models` lists it.
    fn offer_to_download(&self) -> Result<String, String> {
        let text = self.host.models("models offers")?;
        if let Some(why) = text.strip_prefix("err ") {
            return Err(format!("no model is downloaded: {why}"));
        }
        let offers = offered::parse(&text)?;
        let fits = |o: &&offered::Offered| o.fit == Fit::Fits;
        let pick = offers.iter().filter(fits).find(|o| o.recommended);
        match pick.or_else(|| offers.iter().find(fits)) {
            Some(offer) => Ok(offer.id.clone()),
            None => Err(String::from(
                "no signed model can be downloaded on this machine now; models lists each one and why",
            )),
        }
    }

    /// A download of the signed target `name` (P5 task 9): `updated` says
    /// whether it may start and how large it is and from where, and the
    /// shell asks once with that (P5 question 6). It brings gigabytes and has
    /// no undo, so it always asks.
    fn model_download(&mut self, id: u64, request: &str, name: &str) -> Reply {
        let answer = match self.host.models(&format!("models ask {name}")) {
            Ok(answer) => answer,
            Err(why) => return Reply::err(why),
        };
        let Some(subject) = answer.strip_prefix("ok ") else {
            let why = answer.strip_prefix("err ").unwrap_or(&answer);
            return Reply::err(format!("{name} is not downloaded: {why}"));
        };
        let call = download_call(name);
        let waiting = Waiting::Download {
            id: String::from(name),
            subject: String::from(subject),
        };
        let subject = Some(String::from(subject));
        self.model_ask(id, request, &call, waiting, Why::NoUndo, subject)
    }

    /// `model_use` of `name`; a switch that runs is logged with `consent`.
    fn model_use(
        &mut self,
        id: u64,
        request: &str,
        name: &str,
        decision: Decision,
        consent: &str,
    ) -> Reply {
        let models = match self.models_now() {
            Ok(models) => models,
            Err(why) => return Reply::err(why),
        };
        let Some(row) = models.find(name) else {
            return Reply::err(format!("no model {name}: model_list names them"));
        };
        let to = row.sha256;
        if models.use_model(&to).is_none() {
            let why = match models.next() == Some(to) {
                true => "the next restart uses it already",
                false => "it is not a stored model the signed role takes",
            };
            return Reply::err(format!("{}: {why}", row.name()));
        }
        let back = models.next();
        let call = canonical("model_use", name);
        match (decision.tier, decision.why) {
            // A model the AI picked for a vague line asks once.
            (Tier::Run, Why::TakenAs) => self.model_ask(
                id,
                request,
                &call,
                Waiting::Use { to, back },
                Why::TakenAs,
                None,
            ),
            (Tier::Run, _) => {
                let how = How {
                    form: Form::Call,
                    tier: Tier::Run,
                    consent: String::from(consent),
                };
                self.model_switch(id, request, &call, to, back, how)
            }
            (_, why) => self.model_ask(id, request, &call, Waiting::Use { to, back }, why, None),
        }
    }

    /// Asks once for a model row; `subject` puts what it does before the call
    /// ("Download 2.3 GiB from huggingface.co with model_fetch(...)?"), also
    /// when the question is too long for the menu and is asked as y/n.
    fn model_ask(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        waiting: Waiting,
        why: Why,
        subject: Option<String>,
    ) -> Reply {
        self.host.audit(&format!("request {id}: {call} asks"));
        let checked = Checked::shown_only(call);
        let mut reply = self.ask_call(id, request, checked, why, None, subject.clone());
        if let Some(subject) = subject
            && reply.confirm.is_some()
            && reply.text.starts_with("Run ")
        {
            reply.text = format!("{subject}. {}", reply.text);
        }
        if let Some(pending) = &mut self.pending {
            pending.model = Some(waiting);
        }
        reply
    }

    /// The answer to a model row's menu: an Allow whose ONCE checks makes it.
    pub(crate) fn answer_model(&mut self, pending: Pending, text: &str) -> Reply {
        let id = pending.id;
        let Some(waiting) = pending.model.clone() else {
            return Reply::err("nothing to confirm");
        };
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        let how = How {
            consent: String::from("yes"),
            ..pending.how.clone()
        };
        let call = pending.checked.script.clone();
        let reply = match waiting {
            Waiting::Use { to, back } => {
                self.model_switch(id, &pending.request, &call, to, back, how)
            }
            Waiting::Fetch => self.model_fetch(id, &pending.request, &call, how),
            Waiting::Download { id: name, subject } => {
                self.model_download_run(id, &pending.request, &call, how, &name, &subject)
            }
        };
        self.remember(&pending, &reply.text);
        reply
    }

    /// `model_use` made, and logged with the model before as its undo.
    fn model_switch(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        to: Sha256,
        back: Option<Sha256>,
        how: How,
    ) -> Reply {
        let answer = match self.host.models(&format!("models use {}", hex(&to))) {
            Ok(answer) => answer,
            Err(why) => return Reply::err(why),
        };
        if !answer.starts_with("ok ") {
            return Reply::err(answer.trim_start_matches("err ").to_string());
        }
        let name = |sha: Option<Sha256>| sha.map_or_else(|| String::from("none"), |s| hex(&s));
        let change = Change {
            action: String::from("model_use"),
            owner: String::from("updated"),
            res: String::from("model"),
            before: name(back),
            after: name(Some(to)),
            inverse: Some(Inverse::Model { back, from: to }),
        };
        self.log_change(id, request, call, how, change);
        Reply::ok(String::from(
            "Done: the next restart uses it, on trial; the model before comes back if it does not load. undo",
        ))
    }

    fn model_fetch(&mut self, id: u64, request: &str, call: &str, how: How) -> Reply {
        let answer = match self.host.models("models import") {
            Ok(answer) => answer,
            Err(why) => return Reply::err(why),
        };
        let mut words = answer.split(' ');
        let (Some("ok"), Some("importing"), Some(target), Some(length)) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            return Reply::err(answer.trim_start_matches("err ").to_string());
        };
        let length = length.parse().map_or_else(|_| String::from(length), size);
        let change = Change {
            action: String::from("model_fetch"),
            owner: String::from("updated"),
            res: String::from("model-store"),
            before: String::new(),
            after: String::from(target),
            inverse: None,
        };
        self.log_change(id, request, call, how, change);
        Reply::ok(format!(
            "Importing {target} ({length}) from the attached disk, checked against the signed role; model_list shows when it is stored"
        ))
    }

    /// The asked download, started in `updated` after an Allow, and logged:
    /// it brings a new blob and has no undo yet.
    fn model_download_run(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        how: How,
        name: &str,
        subject: &str,
    ) -> Reply {
        let answer = match self.host.models(&format!("models download {name}")) {
            Ok(answer) => answer,
            Err(why) => return Reply::err(why),
        };
        if !answer.starts_with("ok downloading ") {
            return Reply::err(answer.trim_start_matches("err ").to_string());
        }
        let change = Change {
            action: String::from("model_fetch"),
            owner: String::from("updated"),
            res: String::from("model-store"),
            before: String::new(),
            after: String::from(name),
            inverse: None,
        };
        self.log_change(id, request, call, how, change);
        let what = subject.strip_prefix("Download ").unwrap_or(subject);
        Reply::ok(format!(
            "Downloading {name}, {what}; each 2 MiB part is checked against the signed models role before it is kept, and a cut download goes on where it stopped. model_list shows when it is stored"
        ))
    }

    fn log_change(&mut self, id: u64, request: &str, call: &str, how: How, change: Change) {
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: how.form,
            of: None,
            tier: Some(how.tier),
            pack: None,
            consent: how.consent,
            state: State::Done,
            request: String::from(request),
            script: String::from(call),
            outcome: String::from("ran"),
            changes: alloc::vec![change],
        });
    }

    /// `undo` of `model_use`: the model before for the next restart, while the
    /// next restart's model is still `from`, or (`anyway`) whatever it is.
    pub(crate) fn model_back(
        &mut self,
        back: Option<Sha256>,
        from: Sha256,
        anyway: bool,
    ) -> Result<String, Refused> {
        let back = back.ok_or_else(|| {
            Refused::Failed(String::from(
                "no model was in use before, so there is none to go back to",
            ))
        })?;
        let models = self.models_now().map_err(Refused::Failed)?;
        let next = models.next();
        if next != Some(from) && !anyway {
            let now = next.map_or_else(|| String::from("none"), |s| String::from(&hex(&s)[..12]));
            return Err(Refused::Moved(now));
        }
        let answer = self
            .host
            .models(&format!("models use {}", hex(&back)))
            .map_err(Refused::Failed)?;
        match answer.starts_with("ok ") {
            true => Ok(String::from("the model before, from the next restart")),
            false => Err(Refused::Failed(answer)),
        }
    }
}

/// A typed `models` line: what it asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    List,
    Import,
    Use(String),
    Download(String),
}

/// Reads `text` as a `models` line. `None` when its first word is not
/// `models`, so the line goes on to Rhai or the AI; `Some(Err)` when it starts
/// like one but is not right.
pub(crate) fn parse_line(text: &str) -> Option<Result<Line, String>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    Some(match words.as_slice() {
        ["models"] => Ok(Line::List),
        ["models", "import"] => Ok(Line::Import),
        ["models", "use", name] => Ok(Line::Use(String::from(*name))),
        ["models", "download", name] => Ok(Line::Download(String::from(*name))),
        ["models", ..] => Err(String::from(
            "models, models import, models download <id> or models use <id> (models lists \
             the ids); to ask Kai about models, start with ask",
        )),
        _ => return None,
    })
}

impl Shell {
    /// A typed `models` line, held as a typed `install` is: the person's own
    /// line names what it does, through the rows the AI calls. The list
    /// answers, with the signed models this machine may download; a switch
    /// runs, logged as typed, with its undo; an import or a download asks
    /// once, since it has no undo.
    pub(crate) fn models_line(&mut self, request: &str, parsed: Result<Line, String>) -> Reply {
        let line = match parsed {
            Ok(line) => line,
            Err(why) => return Reply::err(why),
        };
        let id = self.next_id;
        self.next_id += 1;
        let typed = |why| Decision {
            tier: Tier::Run,
            why,
        };
        match line {
            Line::List => {
                let mut reply = self.model_call(id, request, "model_list", &[], typed(Why::Read));
                if reply.ok
                    && let Some(offers) = self.offers_text()
                {
                    reply.text = format!("{}\n{offers}", reply.text);
                }
                reply
            }
            Line::Import => self.model_call(id, request, "model_fetch", &[], typed(Why::NoUndo)),
            Line::Use(name) => self.model_use(id, request, &name, typed(Why::Asked), "typed"),
            Line::Download(name) => self.model_download(id, request, &name),
        }
    }

    /// The signed models not stored yet, each with whether this machine may
    /// download it now (`updated`'s `models offers`); `None` when there are
    /// none. Why `updated` could not say is said, not dropped.
    fn offers_text(&self) -> Option<String> {
        let parsed =
            self.host
                .models("models offers")
                .and_then(|text| match text.strip_prefix("err ") {
                    Some(why) => Err(String::from(why)),
                    None => offered::parse(&text),
                });
        let offers = match parsed {
            Ok(offers) => offers,
            Err(why) => return Some(format!("Downloads: none listed ({why})")),
        };
        let lines: Vec<String> = offers
            .iter()
            .filter(|o| o.fit != Fit::Stored)
            .map(|o| {
                let fit = match o.fit {
                    Fit::Fits if o.recommended => String::from("fits, recommended"),
                    Fit::Memory { need_mib, have_mib } => {
                        fetch::Refused::Memory { need_mib, have_mib }.why()
                    }
                    Fit::Disk { need, free } => fetch::Refused::Disk { need, free }.why(),
                    Fit::Fits | Fit::Stored => String::from("fits"),
                };
                format!("  {}: {fit}", o.id)
            })
            .collect();
        if lines.is_empty() {
            return None;
        }
        Some(format!(
            "To download one: models download <id>\n{}",
            lines.join("\n")
        ))
    }
}

/// The canonical call a typed `models download <id>` asks with and logs:
/// `model_fetch("net", "qwen3-1.7b")`.
fn download_call(name: &str) -> String {
    use termcore::once::{CallArg, call_text};
    call_text("model_fetch", &[CallArg::Text("net"), CallArg::Text(name)])
        .unwrap_or_else(|| String::from("model_fetch"))
}

/// The value of the argument `arg` a row was called with, if given.
fn value<'a>(values: &'a [(&'static str, String)], arg: &str) -> Option<&'a str> {
    values
        .iter()
        .find(|(n, _)| *n == arg)
        .map(|(_, v)| v.as_str())
}

/// The canonical call for a model row: `model_use("qwen3-1.7b")`.
fn canonical(action: &str, arg: &str) -> String {
    termcore::once::call_text(action, &[termcore::once::CallArg::Text(arg)])
        .unwrap_or_else(|| String::from(action))
}

/// The list in words, one model a line, as the model manager shows it.
fn listed(models: &Models) -> String {
    let rows = models.rows();
    if rows.is_empty() {
        return String::from("no models: none is stored and the signed role names none");
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let state = match r.state {
                comp_models::State::Running if models.store.on_trial => "in use, on trial",
                comp_models::State::Running => "in use",
                comp_models::State::Next => "from the next restart",
                comp_models::State::Stored => "stored",
                comp_models::State::Available => "signed, not stored",
            };
            let id = r.signed.map_or(String::new(), |s| format!(" ({})", s.id));
            format!(
                "{}{id}: {state}, {}, {}",
                r.name(),
                size(r.length),
                r.licence()
            )
        })
        .collect();
    lines.join("\n")
}
