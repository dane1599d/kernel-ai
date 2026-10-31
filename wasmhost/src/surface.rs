//! A `uikit` window over `kai-surface 1` (ADR 0005 section 3), as comphost
//! presents it: a [`Frame`]'s damage in buffer px for the header page and its
//! accessibility nodes as an `a11y.tree_update`, and back the other way, ring
//! events and seat messages as [`Input`] and `a11y.action_request` as the action
//! `ui_act` takes. Nothing here makes a system call, so it is tested on the host.
//!
//! Two choices the protocol leaves to the client:
//! - `surfabi` refuses node id 0, which is `uikit`'s root, so a node's id on
//!   the wire is its `uikit` id plus one.
//! - `seat.modifiers` carries `uikit`'s [`Mods`] bits (shift 1, ctrl 2, alt 4,
//!   super 8).

use alloc::string::String;
use alloc::vec::Vec;
use surfabi::Rect as Damage;
use surfabi::header::Event;
use surfabi::input::{InputEvent, Kind};
use surfabi::tree::{self, MAX_NAME, MAX_VALUE, Node, Role as WireRole, action, flag};
use uikit::Frame;
use uikit::a11y::{A11yNode, Action, Role};
use uikit::geom::{Point, Rect};
use uikit::input::{Input, Key, Mods};
use uikit::widget::{NodeId, ROOT, Value};

/// `surface.preferred_scale`'s unit: 120 is 1x.
pub const SCALE_ONE: u32 = 120;
/// HID usage of the left button on the button page (0x09).
const LEFT_BUTTON: u16 = 1;

/// A `uikit` node's id on the wire.
pub fn wire_id(node: NodeId) -> u32 {
    node.wrapping_add(1)
}

/// The `uikit` node a wire id names; none for 0.
pub fn node_id(wire: u32) -> Option<NodeId> {
    wire.checked_sub(1)
}

/// The frame's damage in buffer px at `scale` (120 is 1x): each rectangle grown
/// to whole pixels and cut to the buffer. The header page takes at most 32 and
/// bounds the rest itself.
pub fn damage(frame: &Frame, scale: u32) -> Vec<Damage> {
    let (w, h) = frame.size;
    let window = Rect::new(0, 0, w, h);
    frame
        .damage
        .iter()
        .map(|d| window.intersect(d))
        .filter(|d| !d.is_empty())
        .map(|d| {
            let lo = |v: i32| (v.max(0) as u64 * u64::from(scale) / u64::from(SCALE_ONE)) as u32;
            let hi =
                |v: i32| (v.max(0) as u64 * u64::from(scale)).div_ceil(u64::from(SCALE_ONE)) as u32;
            let (x, y) = (lo(d.x), lo(d.y));
            Damage {
                x,
                y,
                w: hi(d.x + d.w) - x,
                h: hi(d.y + d.h) - y,
            }
        })
        .collect()
}

fn role(r: Role) -> WireRole {
    match r {
        Role::Window => WireRole::Window,
        Role::Group => WireRole::Group,
        Role::Label => WireRole::Label,
        Role::Button => WireRole::Button,
        Role::TextInput => WireRole::TextInput,
        Role::MultilineText => WireRole::MultilineTextInput,
        Role::List => WireRole::List,
        Role::Table => WireRole::Table,
        Role::TabList => WireRole::TabList,
        Role::Switch => WireRole::Switch,
        Role::Slider => WireRole::Slider,
        Role::Menu => WireRole::Menu,
        Role::ScrollView => WireRole::ScrollView,
        Role::Split => WireRole::Splitter,
        Role::Image => WireRole::Image,
        Role::Progress => WireRole::ProgressIndicator,
    }
}

/// `uikit`'s actions and their bits on the wire.
const ACTIONS: [(Action, u16); 5] = [
    (Action::Click, action::CLICK),
    (Action::Focus, action::FOCUS),
    (Action::SetValue, action::SET_VALUE),
    (Action::Increment, action::INCREMENT),
    (Action::Decrement, action::DECREMENT),
];

/// `text` as the tree takes it: control characters but `\n` and `\t` dropped,
/// then cut to `max` bytes on a character boundary.
fn tree_text(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

fn node(n: &A11yNode) -> Node {
    let actions = ACTIONS
        .iter()
        .filter(|(a, _)| n.allows(*a))
        .fold(0, |bits, (_, bit)| bits | bit);
    let flags = if n.allows(Action::Focus) {
        flag::FOCUSABLE
    } else {
        0
    };
    Node {
        id: wire_id(n.id),
        role: role(n.role),
        flags,
        actions,
        x: n.bounds.x,
        y: n.bounds.y,
        w: n.bounds.w.max(0) as u32,
        h: n.bounds.h.max(0) as u32,
        name: tree_text(&n.name, MAX_NAME),
        value: tree_text(n.value.as_deref().unwrap_or(""), MAX_VALUE),
        children: n.children.iter().map(|&c| wire_id(c)).collect(),
    }
}

/// The frame's whole tree as one `a11y.tree_update`, focus on the focused node
/// or the window. `desktop` keeps nodes the update names and drops the ones no
/// longer reachable, so a whole tree each frame is a valid update.
pub fn tree_update(frame: &Frame) -> tree::Update {
    let focus = frame.a11y.iter().find(|n| n.focused).map_or(ROOT, |n| n.id);
    tree::Update {
        root: wire_id(ROOT),
        focus: wire_id(focus),
        nodes: frame.a11y.iter().map(node).collect(),
    }
}

/// The [`Mods`] bit of a Shift, Ctrl, Alt or Super key's HID usage (0xE0 to 0xE7,
/// left side then right).
fn modifier_bit(usage: u16) -> Option<u8> {
    match usage {
        0xE1 | 0xE5 => Some(Mods::SHIFT),
        0xE0 | 0xE4 => Some(Mods::CTRL),
        0xE2 | 0xE6 => Some(Mods::ALT),
        0xE3 | 0xE7 => Some(Mods::SUPER),
        _ => None,
    }
}

/// What the glue keeps between seat messages: the modifiers held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seat {
    pub mods: Mods,
}

impl Seat {
    /// `seat.modifiers(mask)`: what is held when the window gains the seat, and
    /// every change after it.
    pub fn modifiers(&mut self, mask: u32) {
        self.mods = Mods((mask & 0xF) as u8);
    }

    /// One ring event as `uikit` input; none for what a window does not take
    /// (frame and buffer events, other buttons, horizontal wheels). A modifier
    /// key's own event changes the modifiers at its place in the ring, as the
    /// seat's message comes by another road: a slow guest can read a Ctrl-B from
    /// the ring before the message that Ctrl is held.
    pub fn input(&mut self, event: &Event) -> Option<Input> {
        let Event::Input {
            event,
            x,
            y,
            text,
            len,
        } = event
        else {
            return None;
        };
        // Surface-local logical px in 24.8 fixed point.
        let at = Point::new(x >> 8, y >> 8);
        let InputEvent {
            kind, code, value, ..
        } = *event;
        Some(match kind {
            Kind::Abs | Kind::Rel => Input::PointerMove(at),
            Kind::Button if code == LEFT_BUTTON => Input::Button {
                at,
                down: value != 0,
                mods: self.mods,
            },
            Kind::Wheel if code == 0 => Input::Wheel { at, dy: value },
            Kind::Key => {
                if let Some(bit) = modifier_bit(code) {
                    self.mods = Mods(if value != 0 {
                        self.mods.0 | bit
                    } else {
                        self.mods.0 & !bit
                    });
                }
                let typed = text.get(..usize::from(*len)).unwrap_or(&[]);
                Input::Key {
                    key: Key::from_hid(code),
                    down: value != 0,
                    mods: self.mods,
                    text: String::from(core::str::from_utf8(typed).unwrap_or("")),
                }
            }
            // The screen changing hands is `desktop`'s, not a window's.
            Kind::Button | Kind::Wheel | Kind::Screen => return None,
        })
    }
}

/// Why an `a11y.action_request` names nothing `ui_act` can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadRequest {
    /// Node id 0, or a node the last frame does not have.
    Node,
    /// An action bit `uikit` has no action for.
    Action,
    /// A value that does not fit the node's role.
    Value,
}

/// `a11y.action_request(node, action, value)` against the last frame's tree:
/// the node, the action (by its bit number) and, for `set_value`, the value as
/// the node's role takes it.
pub fn action_request(
    frame: &Frame,
    node: u32,
    action: u32,
    value: &str,
) -> Result<(NodeId, Action, Option<Value>), BadRequest> {
    let id = node_id(node).ok_or(BadRequest::Node)?;
    let target = frame
        .a11y
        .iter()
        .find(|n| n.id == id)
        .ok_or(BadRequest::Node)?;
    let act = ACTIONS
        .iter()
        .find(|(_, bit)| 1u32.checked_shl(action) == Some(u32::from(*bit)))
        .map(|(a, _)| *a)
        .ok_or(BadRequest::Action)?;
    if act != Action::SetValue {
        return Ok((id, act, None));
    }
    let value = match target.role {
        Role::Switch => match value {
            "true" => Value::On(true),
            "false" => Value::On(false),
            _ => return Err(BadRequest::Value),
        },
        Role::Slider => Value::Number(value.parse().map_err(|_| BadRequest::Value)?),
        Role::List | Role::Table | Role::TabList | Role::Menu => {
            Value::Index(value.parse().map_err(|_| BadRequest::Value)?)
        }
        _ => Value::Text(String::from(value)),
    };
    Ok((id, act, Some(value)))
}
