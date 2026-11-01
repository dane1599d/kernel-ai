//! The scene and its rules (ADR 0005 §1, §3, §8). [`Desk`] takes input and
//! client changes and answers with [`Effect`]s for `desktop` to carry out, so
//! every rule here is tested from recorded input, without a screen.
//!
//! - Windows stack bottom to top. A click raises and focuses a window; its title
//!   bar drags it, its edges resize it (the client is told the new size and
//!   draws it), and its controls minimize, maximize or ask it to close.
//! - Only `desktop` moves keyboard focus: by a click, by Alt+Tab, or by a row
//!   the agent runs. A new window takes focus only when a user action made it
//!   (xdg-activation-v1's rule).
//! - Super, or Ctrl+Space, focuses the AI panel from anywhere.
//! - Every change damages what it uncovers and what it covers.
//! - The trusted layer (ADR 0005 §7): an ask card sits above every window and
//!   takes only y, n or a click on its buttons; Enter does nothing, no window
//!   gets a key or a click while it is up, and keys held before it appeared are
//!   dropped on release. A key or click by the user in the window the agent is
//!   acting on stops the agent ("Stopped: you took over").

use crate::chrome::{self, Edges, PANEL, Part, TOP_BAR};
use crate::damage::Damage;
use crate::geom::{Point, Rect, Size};
use crate::trusted::Ask;
use alloc::string::String;
use alloc::vec::Vec;
/// HID keyboard usages (page 0x07) the desk itself reacts to.
mod ask;
mod manage;

pub mod usage {
    pub const TAB: u16 = 0x2B;
    pub const SPACE: u16 = 0x2C;
    pub const LEFT_CTRL: u16 = 0xE0;
    pub const LEFT_SHIFT: u16 = 0xE1;
    pub const LEFT_ALT: u16 = 0xE2;
    pub const LEFT_GUI: u16 = 0xE3;
    pub const RIGHT_CTRL: u16 = 0xE4;
    pub const RIGHT_SHIFT: u16 = 0xE5;
    pub const RIGHT_ALT: u16 = 0xE6;
    pub const RIGHT_GUI: u16 = 0xE7;
    pub const Y: u16 = 0x1C;
    pub const N: u16 = 0x11;
}
use usage::{SPACE, TAB};
/// How far each new window opens from the last, down and right.
const CASCADE: i32 = 32;
/// The smallest content a resize leaves.
const MIN_CONTENT: Size = Size { w: 120, h: 60 };

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    /// The connection it came from.
    pub client: u32,
    /// The kernel's label for the process, drawn before the title.
    pub label: String,
    pub title: String,
    pub app_id: String,
    /// The outer frame, title bar included.
    pub frame: Rect,
    pub minimized: bool,
    /// The frame to go back to from maximized.
    pub restore: Option<Rect>,
    pub min: Size,
    pub max: Size,
}

impl Window {
    pub fn content(&self) -> Rect {
        chrome::content_of(self.frame)
    }

    pub fn maximized(&self) -> bool {
        self.restore.is_some()
    }
}

/// Where keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Nothing,
    Window(WindowId),
    Panel,
    /// The ask card with this log entry id.
    Ask(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Input in logical px, as `desktop` turns the kernel's events into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Motion(Point),
    Button { button: Button, down: bool },
    Key { usage: u16, down: bool },
}

/// What `desktop` must do after a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The pointer is over `window`'s content at `at` (content-local px).
    Pointer { window: WindowId, at: Point },
    /// The pointer left `window`'s content.
    Leave { window: WindowId },
    Button {
        window: WindowId,
        at: Point,
        button: Button,
        down: bool,
    },
    Key {
        window: WindowId,
        usage: u16,
        down: bool,
    },
    /// A key for the AI panel.
    PanelKey { usage: u16, down: bool },
    /// Keyboard focus moved.
    Focus(Focus),
    /// Tell the client its window's content size and state
    /// (`toplevel.configure`).
    Configure {
        window: WindowId,
        size: Size,
        maximized: bool,
        activated: bool,
    },
    /// The close control was pressed: the client decides (`toplevel.close`).
    Close { window: WindowId },
    /// The user answered the ask card for log entry `id`: y mints its `ONCE`.
    Answer { id: u64, yes: bool },
    /// The user acted in the window the agent was driving: stop the run.
    TookOver { window: WindowId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    Move {
        window: WindowId,
        offset: Point,
    },
    Resize {
        window: WindowId,
        edges: Edges,
        start: Rect,
        from: Point,
    },
}

/// What a point on the screen hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Window(WindowId, Part),
    TopBar,
    Panel,
    Desktop,
}

pub struct Desk {
    width: i32,
    height: i32,
    /// Bottom to top.
    windows: Vec<Window>,
    focus: Focus,
    grab: Option<Grab>,
    pointer: Point,
    /// The window the pointer's content events go to.
    over: Option<WindowId>,
    damage: Damage,
    next: u32,
    /// Modifiers held, by usage.
    ctrl: bool,
    alt: bool,
    shift: bool,
    /// Super went down with no other key since: its release focuses the panel.
    super_alone: bool,
    /// The ask card up now, and where focus goes back to after it.
    ask: Option<(Ask, Focus)>,
    /// Keys down when the card appeared: their releases go nowhere.
    held: Vec<u16>,
    /// Keys down now.
    down: Vec<u16>,
    /// The window the agent is acting on: it has the outline.
    agent: Option<WindowId>,
}

impl Desk {
    /// A desk `width` x `height` logical px, all of it damaged.
    pub fn new(width: i32, height: i32) -> Self {
        let mut damage = Damage::new(width, height);
        damage.add(Rect::new(0, 0, width, height));
        Self {
            width,
            height,
            windows: Vec::new(),
            focus: Focus::Nothing,
            grab: None,
            pointer: Point {
                x: width / 2,
                y: height / 2,
            },
            over: None,
            damage,
            next: 1,
            ctrl: false,
            alt: false,
            shift: false,
            super_alone: false,
            ask: None,
            held: Vec::new(),
            down: Vec::new(),
            agent: None,
        }
    }

    /// Where windows open and maximize: the screen below the top bar and
    /// above the panel.
    pub fn work_area(&self) -> Rect {
        Rect::new(0, TOP_BAR, self.width, self.height - TOP_BAR - PANEL)
    }

    pub fn top_bar(&self) -> Rect {
        Rect::new(0, 0, self.width, TOP_BAR)
    }

    pub fn panel(&self) -> Rect {
        Rect::new(0, self.height - PANEL, self.width, PANEL)
    }

    /// Bottom to top.
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// The window being moved or resized, until it is let go. It is drawn
    /// without its shadow meanwhile (ADR 0005's fail rule for a drag under
    /// load, `desk-perf`'s (f)).
    pub fn dragged(&self) -> Option<WindowId> {
        match self.grab {
            Some(Grab::Move { window, .. } | Grab::Resize { window, .. }) => Some(window),
            None => None,
        }
    }

    pub fn pointer(&self) -> Point {
        self.pointer
    }

    /// The frame's damage: at most 16 rectangles, or their bounding box.
    pub fn take_damage(&mut self) -> Vec<Rect> {
        self.damage.take()
    }

    pub fn has_damage(&self) -> bool {
        !self.damage.is_empty()
    }

    /// Opens a window for `client` with content `size`, cascaded in the work
    /// area. It takes focus only if `activated`: a user action made it.
    pub fn open(
        &mut self,
        client: u32,
        label: &str,
        title: &str,
        size: Size,
        activated: bool,
    ) -> (WindowId, Vec<Effect>) {
        let id = WindowId(self.next);
        self.next += 1;
        let area = self.work_area();
        let n = self.windows.len() as i32 % 8;
        let content = Rect::new(
            area.x + 48 + n * CASCADE + chrome::FRAME,
            area.y + 24 + n * CASCADE + chrome::TITLE_BAR + chrome::FRAME,
            size.w.clamp(MIN_CONTENT.w, area.w.max(MIN_CONTENT.w)),
            size.h.clamp(MIN_CONTENT.h, area.h.max(MIN_CONTENT.h)),
        );
        let window = Window {
            id,
            client,
            label: String::from(label),
            title: String::from(title),
            app_id: String::new(),
            frame: chrome::frame_for(content),
            minimized: false,
            restore: None,
            min: MIN_CONTENT,
            max: Size {
                w: i32::MAX,
                h: i32::MAX,
            },
        };
        self.damage.add(window.frame);
        self.windows.push(window);
        let mut effects = Vec::new();
        if activated {
            self.set_focus(Focus::Window(id), &mut effects);
        }
        (id, effects)
    }

    /// The client is gone or destroyed the window.
    pub fn close(&mut self, id: WindowId) -> Vec<Effect> {
        let mut effects = Vec::new();
        let Some(at) = self.windows.iter().position(|w| w.id == id) else {
            return effects;
        };
        let window = self.windows.remove(at);
        self.damage.add(window.frame);
        if self.over == Some(id) {
            self.over = None;
        }
        if matches!(self.grab, Some(Grab::Move { window, .. } | Grab::Resize { window, .. }) if window == id)
        {
            self.grab = None;
        }
        if self.focus == Focus::Window(id) {
            let next = self.top_visible().map_or(Focus::Nothing, Focus::Window);
            self.set_focus(next, &mut effects);
        }
        effects
    }

    pub fn set_title(&mut self, id: WindowId, title: &str) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.title = String::from(title);
            let bar = chrome::title_bar(w.frame);
            self.damage.add(bar);
        }
    }

    pub fn set_app_id(&mut self, id: WindowId, app_id: &str) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.app_id = String::from(app_id);
        }
    }

    /// The client's size limits, 0 for none (`set_min_size`, `set_max_size`).
    pub fn set_limits(&mut self, id: WindowId, min: Size, max: Size) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.min = Size {
                w: min.w.max(MIN_CONTENT.w),
                h: min.h.max(MIN_CONTENT.h),
            };
            w.max = Size {
                w: if max.w > 0 { max.w } else { i32::MAX },
                h: if max.h > 0 { max.h } else { i32::MAX },
            };
        }
    }

    /// A commit: the client drew `damage` (content-local px) at content
    /// `size`. A new size re-frames the window from its top-left corner.
    pub fn commit(&mut self, id: WindowId, size: Size, damage: &[Rect]) {
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        let content = w.content();
        if size.w != content.w || size.h != content.h {
            let old = w.frame;
            w.frame = chrome::frame_for(Rect::new(content.x, content.y, size.w, size.h));
            let new = w.frame;
            self.damage.add(old);
            self.damage.add(new);
            return;
        }
        if w.minimized {
            return;
        }
        for r in damage {
            if let Some(r) = r.translate(content.x, content.y).intersect(&content) {
                self.damage.add(r);
            }
        }
    }

    /// What `p` hits, topmost first.
    pub fn hit(&self, p: Point) -> Hit {
        for w in self.windows.iter().rev().filter(|w| !w.minimized) {
            if let Some(part) = chrome::hit(w.frame, p) {
                return Hit::Window(w.id, part);
            }
        }
        if self.top_bar().contains(p) {
            Hit::TopBar
        } else if self.panel().contains(p) {
            Hit::Panel
        } else {
            Hit::Desktop
        }
    }

    /// The keys went to the kernel's terminal (INPUT_TO_CONSOLE): the releases
    /// of keys down now go there too, so the desk forgets them, or a key the
    /// desk still thinks held could not answer the next ask card. Modifiers
    /// stay, as they track what the panel's shortcuts see.
    pub fn keys_lent(&mut self) {
        let modifier = |k: &u16| (usage::LEFT_CTRL..=usage::RIGHT_GUI).contains(k);
        self.down.retain(modifier);
        self.held.retain(modifier);
    }

    pub fn input(&mut self, input: Input) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Input::Key { usage: code, down } = input {
            match down {
                true if !self.down.contains(&code) => self.down.push(code),
                true => {}
                false => self.down.retain(|&k| k != code),
            }
        }
        if self.ask.is_some() {
            self.ask_input(input, &mut effects);
            return effects;
        }
        self.takeover(input, &mut effects);
        match input {
            Input::Motion(p) => self.motion(p, &mut effects),
            Input::Button { button, down } => self.button(button, down, &mut effects),
            Input::Key { usage, down } => self.key(usage, down, &mut effects),
        }
        effects
    }

    fn motion(&mut self, p: Point, effects: &mut Vec<Effect>) {
        let p = Point {
            x: p.x.clamp(0, self.width - 1),
            y: p.y.clamp(0, self.height - 1),
        };
        self.pointer = p;
        match self.grab {
            Some(Grab::Move { window, offset }) => {
                let area = self.work_area();
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == window) {
                    let old = w.frame;
                    // The title bar never leaves the work area's top edge.
                    let y = (p.y - offset.y).max(area.y);
                    w.frame = Rect::new(p.x - offset.x, y, old.w, old.h);
                    w.restore = None;
                    let new = w.frame;
                    self.damage.add(old);
                    self.damage.add(new);
                }
                return;
            }
            Some(Grab::Resize {
                window,
                edges,
                start,
                from,
            }) => {
                self.resize(
                    window,
                    edges,
                    start,
                    Point {
                        x: p.x - from.x,
                        y: p.y - from.y,
                    },
                    effects,
                );
                return;
            }
            None => {}
        }
        let over = match self.hit(p) {
            Hit::Window(id, Part::Content(at)) => Some((id, at)),
            _ => None,
        };
        if let Some(old) = self.over
            && over.is_none_or(|(id, _)| id != old)
        {
            effects.push(Effect::Leave { window: old });
        }
        self.over = over.map(|(id, _)| id);
        if let Some((window, at)) = over {
            effects.push(Effect::Pointer { window, at });
        }
    }

    fn button(&mut self, button: Button, down: bool, effects: &mut Vec<Effect>) {
        if !down {
            if let Some(id) = self.dragged() {
                // Let go: its shadow comes back, so its place is drawn again.
                if let Some(frame) = self.window(id).map(|w| w.frame) {
                    self.damage.add(frame);
                }
                self.grab = None;
                return;
            }
            if let Some(window) = self.over
                && let Hit::Window(id, Part::Content(at)) = self.hit(self.pointer)
                && id == window
            {
                effects.push(Effect::Button {
                    window,
                    at,
                    button,
                    down,
                });
            }
            return;
        }
        let hit = self.hit(self.pointer);
        let Hit::Window(id, part) = hit else {
            if hit == Hit::Panel {
                self.set_focus(Focus::Panel, effects);
            }
            return;
        };
        self.raise(id);
        self.set_focus(Focus::Window(id), effects);
        if button != Button::Left {
            if let Part::Content(at) = part {
                effects.push(Effect::Button {
                    window: id,
                    at,
                    button,
                    down,
                });
            }
            return;
        }
        let Some(frame) = self.window(id).map(|w| w.frame) else {
            return;
        };
        match part {
            Part::Content(at) => effects.push(Effect::Button {
                window: id,
                at,
                button,
                down,
            }),
            Part::Title => {
                let offset = Point {
                    x: self.pointer.x - frame.x,
                    y: self.pointer.y - frame.y,
                };
                self.grab = Some(Grab::Move { window: id, offset });
            }
            Part::Edge(edges) => {
                self.grab = Some(Grab::Resize {
                    window: id,
                    edges,
                    start: frame,
                    from: self.pointer,
                });
            }
            Part::Close => effects.push(Effect::Close { window: id }),
            Part::Minimize => self.minimize(id, effects),
            Part::Maximize => self.toggle_maximize(id, effects),
        }
    }

    fn key(&mut self, code: u16, down: bool, effects: &mut Vec<Effect>) {
        match code {
            usage::LEFT_CTRL | usage::RIGHT_CTRL => self.ctrl = down,
            usage::LEFT_ALT | usage::RIGHT_ALT => self.alt = down,
            usage::LEFT_SHIFT | usage::RIGHT_SHIFT => self.shift = down,
            usage::LEFT_GUI | usage::RIGHT_GUI => {
                if down {
                    self.super_alone = true;
                } else if core::mem::take(&mut self.super_alone) {
                    self.set_focus(Focus::Panel, effects);
                }
                return;
            }
            _ => {}
        }
        if down {
            self.super_alone = false;
        }
        if down && code == TAB && self.alt {
            self.cycle(effects);
            return;
        }
        if down && code == SPACE && self.ctrl {
            self.set_focus(Focus::Panel, effects);
            return;
        }
        // Tab and Space released after the shortcut go nowhere.
        if !down && ((code == TAB && self.alt) || (code == SPACE && self.ctrl)) {
            return;
        }
        match self.focus {
            Focus::Window(window) => effects.push(Effect::Key {
                window,
                usage: code,
                down,
            }),
            Focus::Panel => effects.push(Effect::PanelKey { usage: code, down }),
            Focus::Nothing | Focus::Ask(_) => {}
        }
    }

    /// Moves keyboard focus; the old and new windows' title bars repaint.
    pub fn set_focus(&mut self, focus: Focus, effects: &mut Vec<Effect>) {
        if self.focus == focus {
            return;
        }
        for id in [self.focus, focus].into_iter().filter_map(|f| match f {
            Focus::Window(id) => Some(id),
            _ => None,
        }) {
            if let Some(w) = self.window(id) {
                let frame = w.frame;
                self.damage.add(frame.grow(chrome::GRAB));
            }
        }
        // A card takes the keyboard back if anything tries to move it away.
        if let Some((ask, _)) = self.ask
            && focus != Focus::Ask(ask.id)
        {
            return;
        }
        if matches!(self.focus, Focus::Panel) || matches!(focus, Focus::Panel) {
            let panel = self.panel();
            self.damage.add(panel);
        }
        self.focus = focus;
        effects.push(Effect::Focus(focus));
    }
}
