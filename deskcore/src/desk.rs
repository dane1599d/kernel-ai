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

use crate::chrome::{self, Edges, PANEL, Part, TOP_BAR};
use crate::damage::Damage;
use crate::geom::{Point, Rect, Size};
use alloc::string::String;
use alloc::vec::Vec;
/// HID keyboard usages (page 0x07) the desk itself reacts to.
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

    pub fn input(&mut self, input: Input) -> Vec<Effect> {
        let mut effects = Vec::new();
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

    /// Asks the client for the size dragging its `edges` by `delta` gives.
    fn resize(
        &mut self,
        id: WindowId,
        edges: Edges,
        start: Rect,
        delta: Point,
        effects: &mut Vec<Effect>,
    ) {
        let Some(w) = self.window(id) else {
            return;
        };
        let content = chrome::content_of(start);
        let dw = if edges.right {
            delta.x
        } else if edges.left {
            -delta.x
        } else {
            0
        };
        let dh = if edges.bottom {
            delta.y
        } else if edges.top {
            -delta.y
        } else {
            0
        };
        let size = Size {
            w: (content.w + dw).clamp(w.min.w, w.max.w),
            h: (content.h + dh).clamp(w.min.h, w.max.h),
        };
        // Left and top edges move the corner as well as the size.
        let x = if edges.left {
            content.right() - size.w
        } else {
            content.x
        };
        let y = if edges.top {
            content.bottom() - size.h
        } else {
            content.y
        };
        let frame = chrome::frame_for(Rect::new(x, y, content.w, content.h));
        let activated = self.focus == Focus::Window(id);
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            // Moved now; the size changes when the client commits it.
            if w.frame.x != frame.x || w.frame.y != frame.y {
                let old = w.frame;
                w.frame = Rect::new(frame.x, frame.y, old.w, old.h);
                let new = w.frame;
                self.damage.add(old);
                self.damage.add(new);
            }
            w.restore = None;
        }
        effects.push(Effect::Configure {
            window: id,
            size,
            maximized: false,
            activated,
        });
    }

    fn button(&mut self, button: Button, down: bool, effects: &mut Vec<Effect>) {
        if !down {
            if self.grab.take().is_some() {
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
            Focus::Nothing => {}
        }
    }

    /// Alt+Tab: the next window down the stack comes to the top, minimized ones
    /// included, which come back.
    fn cycle(&mut self, effects: &mut Vec<Effect>) {
        if self.windows.is_empty() {
            return;
        }
        let current = match self.focus {
            Focus::Window(id) => self.windows.iter().position(|w| w.id == id),
            _ => None,
        };
        // The bottom window rises: repeated presses walk the whole stack.
        let pick = match current {
            Some(_) if self.windows.len() > 1 => 0,
            Some(at) => at,
            None => self.windows.len() - 1,
        };
        let id = self.windows[pick].id;
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.minimized = false;
            let frame = w.frame;
            self.damage.add(frame);
        }
        self.raise(id);
        self.set_focus(Focus::Window(id), effects);
    }

    fn raise(&mut self, id: WindowId) {
        let Some(at) = self.windows.iter().position(|w| w.id == id) else {
            return;
        };
        if at + 1 == self.windows.len() {
            return;
        }
        let window = self.windows.remove(at);
        self.damage.add(window.frame);
        self.windows.push(window);
    }

    fn minimize(&mut self, id: WindowId, effects: &mut Vec<Effect>) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.minimized = true;
            let frame = w.frame;
            self.damage.add(frame);
        }
        if self.focus == Focus::Window(id) {
            let next = self.top_visible().map_or(Focus::Nothing, Focus::Window);
            self.set_focus(next, effects);
        }
    }

    fn toggle_maximize(&mut self, id: WindowId, effects: &mut Vec<Effect>) {
        let area = self.work_area();
        let activated = self.focus == Focus::Window(id);
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        let (frame, maximized) = match w.restore.take() {
            Some(back) => (back, false),
            None => {
                w.restore = Some(w.frame);
                (area, true)
            }
        };
        // Moved now; the client resizes to the content and commits.
        let content = chrome::content_of(frame);
        let old = w.frame;
        let size = Size {
            w: content.w,
            h: content.h,
        };
        w.frame = Rect::new(frame.x, frame.y, old.w, old.h);
        let new = w.frame;
        self.damage.add(old);
        self.damage.add(new);
        effects.push(Effect::Configure {
            window: id,
            size,
            maximized,
            activated,
        });
    }

    fn top_visible(&self) -> Option<WindowId> {
        self.windows
            .iter()
            .rev()
            .find(|w| !w.minimized)
            .map(|w| w.id)
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
        if matches!(self.focus, Focus::Panel) || matches!(focus, Focus::Panel) {
            let panel = self.panel();
            self.damage.add(panel);
        }
        self.focus = focus;
        effects.push(Effect::Focus(focus));
    }
}
