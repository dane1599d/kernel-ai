//! The agent's window rows on the shell's side (ADR 0005 §6, P3 task 10): what
//! the shell asks `desktop` for ([`Host::ui`]), the window list read back, the
//! State line's `app`, `window` and `windows`, and view-then-act's second
//! round.
//!
//! The second round: a call to `ui_view` is a read whose answer the model needs
//! to finish the request, so the shell reads the view and asks the model once
//! more with it (`llmproto::act::Read`). The view is `Data` (ADR 0002 D6): the
//! call that comes back is decided under a `Content("ui")` control, so a change
//! it makes asks, and it gets no third round.

use crate::state::State;
use crate::undo::{self, Inverse, Name, Refused};
use crate::{Host, Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use llmproto::act::{ActError, ActRequest, Call, Read};
use serde_json::Value;

/// One request to `desktop`, or (`Open`) to the app itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiCall {
    Windows,
    View(String),
    /// Opens `app`: its first request starts it and it opens its window. The
    /// answer is `opened w5` for a new window, or `focused w3 w1` when the
    /// app's window was open already and only took focus (from `w1`).
    Open {
        app: String,
        arg: Option<String>,
    },
    Focus(String),
    /// Puts keyboard focus back where `ui_focus` found it: `w2`, `panel`, `none`.
    Refocus(String),
    Close(String),
    /// Whether `node` is in the window's latest view and takes `action`,
    /// before the call goes to consent: `Err("unknown node; view w3 first")`
    /// for a node no view showed.
    Check {
        window: String,
        node: String,
        action: u32,
    },
    /// `action` is the bit number of `surfabi::tree::action`.
    Act {
        window: String,
        node: String,
        action: u32,
        value: String,
    },
}

/// An open window as `ui_windows` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: String,
    pub app: String,
    pub title: String,
    pub focused: bool,
}

/// `desktop`'s window list, one line a window: `w3\tnotes\tNotes\t1`.
pub fn parse_windows(text: &str) -> Result<Vec<Window>, String> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields[..] {
                [id, app, title, focused @ ("0" | "1")] if id.starts_with('w') => Ok(Window {
                    id: id.to_string(),
                    app: app.to_string(),
                    title: title.to_string(),
                    focused: focused == "1",
                }),
                _ => Err(format!("desktop's window list has a bad line: {line}")),
            }
        })
        .collect()
}

/// The State line's window fields from `desktop`'s list, front to back.
pub fn state_of(windows: &[Window], base: State) -> State {
    let focused = windows.iter().find(|w| w.focused);
    State {
        app: focused.map(|w| w.app.clone()),
        window: focused.map(|w| w.title.clone()),
        windows: windows.iter().map(|w| w.app.clone()).collect(),
        ..base
    }
}

/// `ui_act`'s action word as `surfabi`'s bit number.
pub fn action_bit(action: &str) -> Result<u32, String> {
    agentabi::ui_action_bit(action)
        .ok_or_else(|| format!("{action} is not an action a control takes"))
}

/// What the first round's call comes to: the call to execute, and whether it
/// was decided after reading a view.
pub(crate) enum Round {
    /// Nothing was read: execute it as it came.
    First(Call),
    /// The call the model made after reading a view or a refusal.
    Second(Call),
    /// A read failed, or no call came after it: this is the reply.
    Failed(Reply),
}

/// Reads the model may make before it acts: a view, and a refusal that says
/// to view first (a `ui_act` on a node no view showed). One view at most.
const MAX_READS: usize = 2;

impl Shell {
    /// `call`, or the call the model makes with what it read in front of it: a
    /// view it asked for, or `desktop`'s refusal of a node it guessed, which says
    /// which window to view first (the module's note).
    pub(crate) fn second_round(&mut self, id: u64, asked: &ActRequest, call: Call) -> Round {
        let (mut call, mut reads, mut viewed) = (call, 0, false);
        loop {
            let data = match call.name.as_str() {
                "ui_view" if viewed => {
                    return Round::Failed(Reply::err(
                        "I looked at the window once; say what to do in it",
                    ));
                }
                "ui_view" => match self.read_view(&call) {
                    Ok(view) => {
                        viewed = true;
                        view
                    }
                    Err(reply) => return Round::Failed(reply),
                },
                "ui_act" => match self.unknown_node(&call) {
                    Some(refusal) if reads == MAX_READS => {
                        return Round::Failed(Reply::err(refusal));
                    }
                    Some(refusal) => refusal,
                    None if reads == 0 => return Round::First(call),
                    None => return Round::Second(call),
                },
                _ if reads == 0 => return Round::First(call),
                // These do not go through a row's consent, so what was read
                // could steer them past it: after a read only rows, `say` and
                // `clarify` come.
                "script" | "undo" => {
                    return Round::Failed(Reply::err(format!(
                        "after looking at a window I act only in it; ask for {} on its own",
                        call.name
                    )));
                }
                _ => return Round::Second(call),
            };
            if reads == MAX_READS {
                return Round::Failed(Reply::err(
                    "I looked at the window once; say what to do in it",
                ));
            }
            reads += 1;
            self.host.audit(&format!(
                "request {id}: {} read for round {}",
                call.name,
                reads + 1
            ));
            let again = ActRequest {
                read: Some(Read {
                    call: call.text(),
                    data,
                }),
                ..asked.clone()
            };
            call = match self.host.act(&again) {
                Ok(next) => {
                    self.host.audit(&format!(
                        "request {id}: then the model called {}",
                        next.text()
                    ));
                    next
                }
                Err(error @ (ActError::Local(_) | ActError::CutOff | ActError::NotACall)) => {
                    return Round::Failed(Reply::err(format!(
                        "the model gave no call after reading the window: {}",
                        error.describe()
                    )));
                }
                Err(ActError::Unavailable(why)) => return Round::Failed(Reply::err(why)),
            };
        }
    }

    /// The view `call` (a `ui_view`) asks for, or why there is none.
    fn read_view(&self, call: &Call) -> Result<String, Reply> {
        let window = text(call, "window");
        self.host
            .ui(&UiCall::View(window.into()))
            .map_err(|why| Reply::err(format!("I could not look at {window}: {why}")))
    }

    /// `desktop`'s refusal of `call` (a `ui_act`) when its node is in no view
    /// of the window: "unknown node; view w3 first". `None` when the node is
    /// known, or when only the act itself can say (no desktop, a bad action):
    /// the call then goes on to consent as it came.
    fn unknown_node(&self, call: &Call) -> Option<String> {
        let check = UiCall::Check {
            window: text(call, "window").into(),
            node: text(call, "node").into(),
            action: action_bit(text(call, "action")).ok()?,
        };
        match self.host.ui(&check) {
            Err(why) if why.starts_with("unknown node") => Some(why),
            _ => None,
        }
    }
}

fn text<'a>(call: &'a Call, name: &str) -> &'a str {
    call.arguments
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// What undoes a `ui_open` that answered `answer` for `app`: closing the
/// window it opened, or, when it only focused a window the user had, putting
/// focus back. `None` for an answer that says neither.
pub(crate) fn opened(answer: &str, app: &str) -> Option<Inverse> {
    let mut words = answer.split(' ');
    let inverse = match (words.next()?, words.next()?, words.next()) {
        ("opened", window, None) => Inverse::Reclose {
            window: Name::new(window)?,
            app: Name::new(app)?,
        },
        ("focused", window, Some(before)) => Inverse::Refocus {
            to: Name::new(before)?,
            from: Name::new(window)?,
        },
        _ => return None,
    };
    words.next().is_none().then_some(inverse)
}

/// Puts back a window change (`undo`): what it did and its redo, or why not.
/// Closing a window the agent opened asks first unless `anyway`: the user may
/// have worked in it since.
pub(crate) fn revert(
    host: &dyn Host,
    inverse: Inverse,
    anyway: bool,
) -> Result<(String, Inverse), Refused> {
    match inverse {
        Inverse::Refocus { to, from } => host
            .ui(&UiCall::Refocus(to.as_str().into()))
            .map(|_| {
                let text = format!("focus back on {}", to.as_str());
                (text, Inverse::Refocus { to: from, from: to })
            })
            .map_err(Refused::Failed),
        Inverse::Reopen(app) => {
            let open = UiCall::Open {
                app: app.as_str().into(),
                arg: None,
            };
            let answer = host.ui(&open).map_err(Refused::Failed)?;
            let redo = opened(&answer, app.as_str()).unwrap_or(undo::reverse(inverse));
            Ok((format!("{} opened again", app.as_str()), redo))
        }
        Inverse::Reclose { .. } if !anyway => Err(Refused::Moved(String::from(
            "open, and it may hold unsaved work",
        ))),
        Inverse::Reclose { window, app } => host
            .ui(&UiCall::Close(window.as_str().into()))
            .map(|_| {
                (
                    format!("{} closed again", app.as_str()),
                    Inverse::Reopen(app),
                )
            })
            .map_err(Refused::Failed),
        _ => Err(Refused::Failed(String::from("it is not a window change"))),
    }
}

/// The host's `ui`, the default for a host with no desktop.
pub(crate) fn no_desktop() -> Result<String, String> {
    Err(String::from("there is no desktop here"))
}

/// What `host` says the open windows are.
pub fn windows(host: &dyn Host) -> Result<Vec<Window>, String> {
    parse_windows(&host.ui(&UiCall::Windows)?)
}
