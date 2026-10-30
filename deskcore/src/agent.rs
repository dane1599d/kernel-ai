//! The agent's window rows as `desktop` answers them (ADR 0005 §6, P3 task 10).
//! Calls go one way: the shell asks, `desktop` answers, one request a call, as
//! text. A window is named by its id (`w3`) or its app (`notes`, the frontmost
//! of that app's windows):
//!
//! - `ui windows` → one line a window, front to back: `w3\tnotes\tNotes\t1`,
//!   the last field 1 for the focused window;
//! - `ui view <window>` → `ok <view>`: [`crate::view::project`]'s text. The
//!   view is kept, so `ui act` reads its short ids as the model did;
//! - `ui focus <window>` → `ok <before>`, where keyboard focus was (`w2`,
//!   `panel` or `none`), for `undo`; `ui refocus <before>` puts it back;
//! - `ui close <window>` → `ok <app>`: the app is asked to close it, as by its
//!   close control, and decides;
//! - `ui act <window> <node> <action>\t<value>` → `ok`: an AccessKit
//!   `ActionRequest` to the app for a node of the window's last view, never
//!   synthetic input. The agent's outline moves to the window. A node no
//!   view of the window showed is `err unknown node; view w3 first`;
//! - `ui check <window> <node> <action>` → `ok`, or `ui act`'s refusal: the
//!   shell asks before it puts a call to the user;
//! - `ui agent <window>` or `ui agent none` → `ok`: the outline while a run
//!   drives a window, or none. A key or click by the user there stops the run
//!   ([`crate::desk::Effect::TookOver`], [`Agent::took_over`]): from then on
//!   `ui act`, `ui check`, `ui focus`, `ui close` and `ui agent <window>` are
//!   refused with `err stopped: you took over`, until the shell's next request
//!   begins with `ui agent none`.
//!
//! A private window (`toplevel.set_agent_private`, ADR 0005 §6) is kept from
//! the agent: `ui view` answers the window line and `w3: private` with no
//! nodes, and `ui act`, `ui check`, `ui focus` and a `ui refocus` into it are
//! refused. It is asked at each request, so a window that turns private later
//! is kept from then on, and a view taken before cannot be acted on.
//!
//! Anything else is `err <why>`, in words the shell shows. `desktop` takes these
//! from the shell alone.

use crate::desk::{Desk, Effect, Focus, WindowId};
use crate::view::{self, Listed, View};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use surfabi::tree::Tree;

/// The longest value `ui act` carries, as `agentabi`'s `ui_act` row allows.
pub const MAX_VALUE: usize = 512;

/// One request, read from the shell's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Windows,
    View(String),
    Focus(String),
    Refocus(String),
    Close(String),
    Act {
        window: String,
        node: String,
        action: u32,
        value: String,
    },
    Check {
        window: String,
        node: String,
        action: u32,
    },
    Agent(Option<String>),
}

impl Request {
    pub fn encode(&self) -> String {
        match self {
            Self::Windows => "ui windows".into(),
            Self::View(w) => format!("ui view {w}"),
            Self::Focus(w) => format!("ui focus {w}"),
            Self::Refocus(before) => format!("ui refocus {before}"),
            Self::Close(w) => format!("ui close {w}"),
            Self::Act {
                window,
                node,
                action,
                value,
            } => format!("ui act {window} {node} {action}\t{value}"),
            Self::Check {
                window,
                node,
                action,
            } => format!("ui check {window} {node} {action}"),
            Self::Agent(Some(w)) => format!("ui agent {w}"),
            Self::Agent(None) => "ui agent none".into(),
        }
    }

    /// [`Request::encode`] read back; `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("ui ")?;
        let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        let word = |w: &str| !w.is_empty() && w.len() <= 24 && !w.contains(char::is_whitespace);
        Some(match verb {
            "windows" if arg.is_empty() => Self::Windows,
            "view" if word(arg) => Self::View(arg.into()),
            "focus" if word(arg) => Self::Focus(arg.into()),
            "refocus" if word(arg) => Self::Refocus(arg.into()),
            "close" if word(arg) => Self::Close(arg.into()),
            "agent" if arg == "none" => Self::Agent(None),
            "agent" if word(arg) => Self::Agent(Some(arg.into())),
            "check" => {
                let mut words = arg.split(' ');
                let (window, node, action) = (words.next()?, words.next()?, words.next()?);
                if words.next().is_some() || !word(window) || !word(node) {
                    return None;
                }
                Self::Check {
                    window: window.into(),
                    node: node.into(),
                    action: action.parse().ok().filter(|a| *a < 16)?,
                }
            }
            "act" => {
                let (head, value) = arg.split_once('\t')?;
                let mut words = head.split(' ');
                let (window, node, action) = (words.next()?, words.next()?, words.next()?);
                let fits = word(window) && word(node) && value.len() <= MAX_VALUE;
                if words.next().is_some() || !fits || value.contains('\n') {
                    return None;
                }
                Self::Act {
                    window: window.into(),
                    node: node.into(),
                    action: action.parse().ok().filter(|a| *a < 16)?,
                    value: value.into(),
                }
            }
            _ => return None,
        })
    }
}

/// What `desktop` keeps for the agent: each window's last view, and whether
/// the user took over from the run.
#[derive(Default)]
pub struct Agent {
    views: Vec<(WindowId, View)>,
    stopped: bool,
}

/// The refusal while the user has taken over (ADR 0005 §6).
pub const STOPPED: &str = "stopped: you took over";

/// What answering a request needs of `desktop` beyond the desk: each window's
/// tree, whether its app keeps it from the agent, and sending an
/// `ActionRequest` to its app.
pub trait Clients {
    fn tree(&self, window: WindowId) -> Option<&Tree>;
    /// Whether `window`'s app keeps it from the agent
    /// (`toplevel.set_agent_private`, [`crate::server::Client::agent_private`]).
    /// Asked at every request, so a window that turns private after its tree
    /// came is kept from then on.
    fn private(&self, window: WindowId) -> bool;
    /// Whether the app was asked: the node offers the action.
    fn act(&mut self, window: WindowId, node: u32, action: u32, value: &str) -> bool;
}

/// What `ui view` of a private window says after its window line, in place of
/// its nodes.
pub const PRIVATE: &str = "private";

/// The refusal for acting in, or focusing, a private window.
fn kept_private(name: &str) -> String {
    format!("{name} is private: its app keeps it from the agent")
}

impl Agent {
    /// The answer to `text`, and the effects `desktop` carries out (focus,
    /// close).
    pub fn answer(
        &mut self,
        desk: &mut Desk,
        clients: &mut dyn Clients,
        text: &str,
    ) -> (String, Vec<Effect>) {
        let mut effects = Vec::new();
        let answer = match Request::parse(text) {
            Some(request) => self.run(desk, clients, request, &mut effects),
            None => Err(String::from("desktop does not know that request")),
        };
        let text = match answer {
            Ok(text) => text,
            Err(why) => format!("err {why}"),
        };
        (text, effects)
    }

    fn run(
        &mut self,
        desk: &mut Desk,
        clients: &mut dyn Clients,
        request: Request,
        effects: &mut Vec<Effect>,
    ) -> Result<String, String> {
        let acts = matches!(
            request,
            Request::Act { .. }
                | Request::Check { .. }
                | Request::Focus(_)
                | Request::Close(_)
                | Request::Agent(Some(_))
        );
        if self.stopped && acts {
            return Err(String::from(STOPPED));
        }
        let find = |desk: &Desk, name: &str| {
            find_window(desk, name).ok_or_else(|| format!("no window is {name}"))
        };
        match request {
            Request::Windows => Ok(windows_text(&view::listed(desk))),
            Request::View(name) => {
                let window = find(desk, &name)?;
                if clients.private(window) {
                    // Its title is in the window line; no node, and no view
                    // kept, so no short id from before can be acted on.
                    self.views.retain(|(w, _)| *w != window);
                    let list = view::windows_line(&view::listed(desk));
                    return Ok(format!("ok {list}\n{}: {PRIVATE}", view::window_id(window)));
                }
                let empty = Tree::default();
                let tree = clients.tree(window).unwrap_or(&empty);
                let view = view::project(&view::listed(desk), window, tree);
                let text = format!("ok {}", view.text);
                self.views.retain(|(w, _)| *w != window);
                self.views.push((window, view));
                Ok(text)
            }
            Request::Focus(name) => {
                let window = find(desk, &name)?;
                if clients.private(window) {
                    return Err(kept_private(&name));
                }
                let before = focus_name(desk.focus());
                desk.activate(window, effects);
                // The agent drives the window it moved to: it has the outline.
                desk.set_agent(Some(window));
                Ok(format!("ok {before}"))
            }
            Request::Refocus(before) => {
                match before.as_str() {
                    "panel" => desk.set_focus(Focus::Panel, effects),
                    "none" => desk.set_focus(Focus::Nothing, effects),
                    // An undo puts focus back, but never into a private window.
                    name => {
                        let window = find(desk, name)?;
                        if clients.private(window) {
                            return Err(kept_private(name));
                        }
                        desk.activate(window, effects)
                    }
                }
                Ok(String::from("ok"))
            }
            Request::Close(name) => {
                let window = find(desk, &name)?;
                let app = view::listed(desk)
                    .into_iter()
                    .find(|l| l.id == window)
                    .map(|l| l.app)
                    .unwrap_or_default();
                effects.push(Effect::Close { window });
                Ok(format!("ok {app}"))
            }
            Request::Act {
                window,
                node,
                action,
                value,
            } => {
                let window = find(desk, &window)?;
                let id = self.checked(&*clients, window, &node, action)?;
                if !clients.act(window, id, action, &value) {
                    return Err(format!("{node} does not take that action"));
                }
                desk.set_agent(Some(window));
                Ok(String::from("ok"))
            }
            Request::Check {
                window,
                node,
                action,
            } => {
                let window = find(desk, &window)?;
                self.checked(&*clients, window, &node, action)?;
                Ok(String::from("ok"))
            }
            Request::Agent(None) => {
                // The shell's next request: a new run.
                self.stopped = false;
                desk.set_agent(None);
                Ok(String::from("ok"))
            }
            Request::Agent(name) => {
                let window = name.map(|n| find(desk, &n)).transpose()?;
                desk.set_agent(window);
                Ok(String::from("ok"))
            }
        }
    }

    /// The tree's id of `node` in `window`'s latest view, if it offers
    /// `action`. A node no view showed (an id the model guessed) is unknown, and
    /// the answer says what to do instead.
    fn checked(
        &self,
        clients: &dyn Clients,
        window: WindowId,
        node: &str,
        action: u32,
    ) -> Result<u32, String> {
        if clients.private(window) {
            return Err(kept_private(&view::window_id(window)));
        }
        let id = self
            .views
            .iter()
            .find(|(w, _)| *w == window)
            .and_then(|(_, v)| v.node(node))
            .ok_or_else(|| format!("unknown node; view {} first", view::window_id(window)))?;
        let offers = clients
            .tree(window)
            .and_then(|tree| tree.node(id))
            .is_some_and(|n| action < 16 && n.actions & (1 << action) != 0);
        match offers {
            true => Ok(id),
            false => Err(format!("{node} does not take that action")),
        }
    }

    /// The user acted in the window the agent was driving: the rest of the run
    /// acts on nothing.
    pub fn took_over(&mut self) {
        self.stopped = true;
    }

    /// A window closed: its view goes.
    pub fn closed(&mut self, window: WindowId) {
        self.views.retain(|(w, _)| *w != window);
    }
}

/// `w3` by its id, or an app's frontmost window by the app's name (or the
/// title, when no app has that name), case aside.
pub fn find_window(desk: &Desk, name: &str) -> Option<WindowId> {
    let listed = view::listed(desk);
    if let Some(id) = view::parse_window(name) {
        return listed.iter().any(|l| l.id == id).then_some(id);
    }
    let by = |key: fn(&Listed) -> &str| {
        listed
            .iter()
            .find(|l| key(l).eq_ignore_ascii_case(name))
            .map(|l| l.id)
    };
    by(|l| &l.app).or_else(|| by(|l| &l.title))
}

/// `ui windows`' answer: one line a window, its fields without tabs or breaks.
pub fn windows_text(listed: &[Listed]) -> String {
    let field = |text: &str| text.replace(['\t', '\n', '\r'], " ");
    let lines: Vec<String> = listed
        .iter()
        .map(|l| {
            format!(
                "{}\t{}\t{}\t{}",
                view::window_id(l.id),
                field(&l.app),
                field(&l.title),
                u8::from(l.focused)
            )
        })
        .collect();
    lines.join("\n")
}

/// [`windows_text`] read back: (id, app, title, focused).
pub fn parse_windows(text: &str) -> Option<Vec<(String, String, String, bool)>> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let (id, app, title, focused) = (
                fields.next()?,
                fields.next()?,
                fields.next()?,
                fields.next()?,
            );
            let focused = match focused {
                "1" => true,
                "0" => false,
                _ => return None,
            };
            (fields.next().is_none() && view::parse_window(id).is_some())
                .then(|| (id.to_string(), app.to_string(), title.to_string(), focused))
        })
        .collect()
}

fn focus_name(focus: Focus) -> String {
    match focus {
        Focus::Window(id) => view::window_id(id),
        Focus::Panel => String::from("panel"),
        Focus::Nothing | Focus::Ask(_) => String::from("none"),
    }
}
