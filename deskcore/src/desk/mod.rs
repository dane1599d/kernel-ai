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
//! - A Super tap, or Ctrl+Space, opens the Kai pane and focuses its input
//!   from anywhere ([`pane`]); the pane is hit before the windows.
//! - Every change damages what it uncovers and what it covers.
//! - The trusted layer (ADR 0005 §7): an ask card sits above every window and
//!   takes only y, n or a click on its buttons; Enter does nothing, no window
//!   gets a key or a click while it is up, and keys held before it appeared are
//!   dropped on release. A key or click by the user in the window the agent is
//!   acting on stops the agent ("Stopped: you took over").

use crate::chrome::{self, Part};
use crate::damage::Damage;
use crate::geom::{Point, Rect, Size};
use crate::trusted::Ask;
use alloc::string::String;
use alloc::vec::Vec;
use deskgeom::{Layout, UiState};
/// HID keyboard usages (page 0x07) the desk itself reacts to.
mod ask;
mod bar;
mod keys;
mod manage;
mod pane;
mod start;
pub use bar::{BarSpot, TrayPart};
pub use pane::PanePart;
pub use start::TIP_MS;

mod input;
mod types;
pub use types::*;

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
    /// The title-bar control under the pointer, drawn with its hover fill.
    hovered: Option<(WindowId, Part)>,
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
    /// The Kai pane is open.
    pane_open: bool,
    /// Where the focus was before the pane's input took it: Esc gives it
    /// back.
    before_panel: Focus,
    /// Keys down when the card appeared: their releases go nowhere.
    held: Vec<u16>,
    /// Keys down now.
    down: Vec<u16>,
    /// The window the agent is acting on: it has the outline.
    agent: Option<WindowId>,
    /// Caps Lock: letters type the other case from Shift.
    caps: bool,
    /// Num Lock: on, the keypad types digits; off, it is the arrows and
    /// navigation keys. On at start, as most PCs boot.
    num: bool,
    /// The taskbar's spot under the pointer, drawn with its hover fill.
    bar_hover: Option<BarSpot>,
    /// Kai said something while the pane was closed: the chip's badge.
    unread: bool,
    /// Start is open.
    start_open: bool,
    /// Where the keys were when Start opened: they go back there when it
    /// closes without a pick.
    start_from: Focus,
    /// Start's spot under the pointer (a cell, recent row, search row or
    /// All apps row, by geometry): its hover repaints only as it moves.
    start_spot: [i32; 4],
    /// The apps installed, by name: the taskbar's pins.
    installed: Vec<String>,
    /// When the pointer came to the taskbar spot it rests on, and whether
    /// that item's tooltip shows.
    hover_since: u64,
    tip_shown: bool,
    /// The clock `desktop` last gave ([`Desk::set_now`]).
    now_ms: u64,
    /// When the card up now may first be answered: a quiet
    /// [`termcore::menu::SETTLE_MS`] after it was first drawn and after the
    /// last key or button pressed; `u64::MAX` until it is drawn.
    ask_open_ms: u64,
    /// When the card up now was first drawn ([`Desk::card_drawn`]).
    ask_drawn: Option<u64>,
    /// The last key or button pressed while the card was up.
    ask_pressed_ms: u64,
    /// The pointer moved onto the card's Run button after the card was
    /// drawn: a click there may run it (one parked there before may not).
    ask_entered_run: bool,
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
            hovered: None,
            damage,
            next: 1,
            ctrl: false,
            alt: false,
            shift: false,
            super_alone: false,
            ask: None,
            pane_open: false,
            before_panel: Focus::Nothing,
            held: Vec::new(),
            down: Vec::new(),
            agent: None,
            caps: false,
            num: true,
            bar_hover: None,
            unread: false,
            start_open: false,
            start_from: Focus::Nothing,
            start_spot: [-1; 4],
            installed: Vec::new(),
            hover_since: 0,
            tip_shown: false,
            now_ms: 0,
            ask_open_ms: u64::MAX,
            ask_drawn: None,
            ask_pressed_ms: 0,
            ask_entered_run: false,
        }
    }

    /// The clock, in ms, for the card's settle time: `desktop` gives each
    /// input's own event time before it, so keys are judged by when they were
    /// pressed, not by when they were read.
    pub fn set_now(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    /// Whether a window `client` opens now may take the keyboard focus (the
    /// activation rule of ADR 0005 §3): only when nothing has it (the desk at
    /// boot, after a click on it) or one of that client's own windows has it.
    /// Any other window opens without the keys, however long the person has been
    /// still: what the person asked for (`open notes` through the shell, a pin,
    /// Start) is given the focus by the shell's `ui focus`, which `ui_open`
    /// sends once the window is on the desk, so a background app cannot take the
    /// keys meant for the pane, the Terminal or another app.
    pub fn may_activate(&self, client: u32) -> bool {
        match self.focus {
            Focus::Nothing => true,
            Focus::Window(id) => self.window(id).is_some_and(|w| w.client == client),
            _ => false,
        }
    }

    /// Where windows open: the screen above the taskbar.
    pub fn work_area(&self) -> Rect {
        self.layout().work_area
    }

    /// Every surface's rectangle at this screen size (deskgeom).
    pub fn layout(&self) -> Layout {
        let ui = UiState {
            pane_open: self.pane_open,
        };
        Layout::for_screen(self.width, self.height, ui)
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

    /// The title-bar control the pointer is over, if any: minimize,
    /// maximize or close.
    pub fn hovered(&self) -> Option<(WindowId, Part)> {
        self.hovered
    }

    pub fn pointer(&self) -> Point {
        self.pointer
    }

    /// The frame's damage: at most 16 rectangles, or their bounding box. The
    /// hover is settled first, so a frame never paints a stale one.
    pub fn take_damage(&mut self) -> Vec<Rect> {
        self.rehover();
        self.damage.take()
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
        let layout = self.layout();
        let area = layout.work_area;
        let at = layout.cascade(self.windows.len() as i32);
        let content = Rect::new(
            at.x + chrome::FRAME,
            at.y + chrome::TITLE_BAR + chrome::FRAME,
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
        self.bar_changed();
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
        self.bar_changed();
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
            self.bar_changed();
            // A tooltip up shows the title: it repaints with it.
            if self.tip_shown {
                self.tip_repaint();
            }
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
        // Never narrower than the desk allows, whatever the client drew.
        let size = Size {
            w: size.w.max(MIN_CONTENT.w),
            h: size.h.max(MIN_CONTENT.h),
        };
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

    /// What `p` hits, topmost first (ADR 0005 §7's hit order): the pane,
    /// then the taskbar and tray, are drawn over every window, so a window
    /// under them never takes their clicks.
    pub fn hit(&self, p: Point) -> Hit {
        if self.start().is_some_and(|r| r.contains(p)) {
            return Hit::Start;
        }
        if let Some(part) = self.pane_hit(p) {
            return Hit::Pane(part);
        }
        if let Some(hit) = self.bar_hit(p) {
            return hit;
        }
        for w in self.windows.iter().rev().filter(|w| !w.minimized) {
            let part = match w.maximized() {
                true => chrome::hit_maximized(w.frame, p),
                false => chrome::hit(w.frame, p),
            };
            if let Some(part) = part {
                return Hit::Window(w.id, part);
            }
        }
        Hit::Desktop
    }

    /// Whether Caps Lock is on, for the panel's own keymap.
    pub fn caps(&self) -> bool {
        self.caps
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
        // A key-down for a key already down is its repeat, never a fresh press.
        let repeat =
            matches!(input, Input::Key { usage, down: true } if self.down.contains(&usage));
        if let Input::Key { usage: code, down } = input {
            match down {
                true if !self.down.contains(&code) => self.down.push(code),
                true => {}
                false => self.down.retain(|&k| k != code),
            }
        }
        if self.ask.is_some() {
            self.ask_input(input, repeat, &mut effects);
        } else {
            self.takeover(input, &mut effects);
            match input {
                Input::Motion(p) => self.motion(p, &mut effects),
                Input::Button { button, down } => self.button(button, down, &mut effects),
                Input::Key { usage, down } => self.key(usage, down, &mut effects),
                Input::Wheel(detents) => self.wheel(detents, &mut effects),
            }
        }
        // A click, a key or a card may have moved what is under the pointer.
        self.rehover();
        effects
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
        let from = self.focus;
        self.focus = focus;
        self.bar_changed();
        // Start is the panel's focus' other host: it goes as the focus
        // leaves, to a card included.
        if focus != Focus::Panel {
            self.close_start(effects);
        }
        effects.push(Effect::Focus(focus));
        self.pane_focus(from, focus, effects);
    }
}
