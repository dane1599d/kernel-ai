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
    /// Opens `app`: its first request starts it and it opens its window.
    Open {
        app: String,
        arg: Option<String>,
    },
    Focus(String),
    /// Puts keyboard focus back where `ui_focus` found it: `w2`, `panel`, `none`.
    Refocus(String),
    Close(String),
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
    /// Not a view: execute it as it came.
    First(Call),
    /// The call the model made after reading the view.
    Second(Call),
    /// The view could not be read, or no second call came: this is the reply.
    Failed(Reply),
}

impl Shell {
    /// `call`, or, if it reads a view, the call the model makes with the view
    /// in front of it (the module's note).
    pub(crate) fn second_round(&mut self, id: u64, asked: &ActRequest, call: Call) -> Round {
        if call.name != "ui_view" {
            return Round::First(call);
        }
        let window = call
            .arguments
            .get("window")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let view = match self.host.ui(&UiCall::View(window.clone())) {
            Ok(view) => view,
            Err(why) => {
                return Round::Failed(Reply::err(format!("I could not look at {window}: {why}")));
            }
        };
        self.host.audit(&format!(
            "request {id}: read the view of {window} for a second round"
        ));
        let again = ActRequest {
            read: Some(Read {
                call: call.text(),
                data: view,
            }),
            ..asked.clone()
        };
        match self.host.act(&again) {
            Ok(next) if next.name == "ui_view" => Round::Failed(Reply::err(
                "I looked at the window once; say what to do in it",
            )),
            Ok(next) => {
                self.host.audit(&format!(
                    "request {id}: after the view, the model called {}",
                    next.text()
                ));
                Round::Second(next)
            }
            Err(error @ (ActError::Local(_) | ActError::CutOff | ActError::NotACall)) => {
                Round::Failed(Reply::err(format!(
                    "the model gave no call after the view: {}",
                    error.describe()
                )))
            }
            Err(ActError::Unavailable(why)) => Round::Failed(Reply::err(why)),
        }
    }
}

/// Puts back a window change (`undo`): what it did, or why not.
pub(crate) fn revert(host: &dyn Host, inverse: crate::undo::Inverse) -> Result<String, String> {
    use crate::undo::Inverse;
    match inverse {
        Inverse::Refocus { to, .. } => host
            .ui(&UiCall::Refocus(to.as_str().into()))
            .map(|_| format!("focus back on {}", to.as_str())),
        Inverse::Reopen(app) => host
            .ui(&UiCall::Open {
                app: app.as_str().into(),
                arg: None,
            })
            .map(|_| format!("{} opened again", app.as_str())),
        Inverse::Reclose(app) => host
            .ui(&UiCall::Close(app.as_str().into()))
            .map(|_| format!("{} closed again", app.as_str())),
        _ => Err(String::from("it is not a window change")),
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
