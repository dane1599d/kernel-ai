//! The pointer's motion and buttons. A press in a window's content holds the
//! pointer for that window until the last button is up (an implicit grab, as
//! X and Wayland have): it hears the release wherever it comes.

use super::*;

impl Desk {
    pub(super) fn motion(&mut self, p: Point, effects: &mut Vec<Effect>) {
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
                    // The title bar stays in the work area: never above its
                    // top, never under the panel, and enough of it on screen
                    // to grab it again from either side.
                    let y = (p.y - offset.y)
                        .min(area.bottom() - chrome::TITLE_BAR)
                        .max(area.y);
                    let x = (p.x - offset.x)
                        .min(area.right() - KEEP_ON_SCREEN)
                        .max(area.x + KEEP_ON_SCREEN - old.w);
                    w.frame = Rect::new(x, y, old.w, old.h);
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
        if let Some((window, _)) = self.pressed {
            if let Some(at) = self.content_at(window, p) {
                effects.push(Effect::Pointer { window, at });
                return;
            }
            // Its window is gone: nothing holds the pointer.
            self.pressed = None;
        }
        self.hover_content(p, effects);
    }

    /// The pointer at `p` with no button held in a window: which content it
    /// is over, a `Leave` for the one it left.
    fn hover_content(&mut self, p: Point, effects: &mut Vec<Effect>) {
        let hit = self.hit(p);
        let over = match hit {
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

    pub(super) fn button(&mut self, button: Button, down: bool, effects: &mut Vec<Effect>) {
        if !down {
            if let Some(id) = self.dragged() {
                // Let go: its shadow comes back, so its place is drawn again.
                if let Some(frame) = self.window(id).map(|w| w.frame) {
                    self.damage.add(frame);
                }
                self.grab = None;
                return;
            }
            // The window the press went to hears its release, in or out of
            // its content; once the last button is up the pointer is over
            // what it is over again.
            if let Some((window, held)) = self.pressed {
                // A button whose press went elsewhere: its release is not
                // the window's either.
                if held & bit(button) == 0 {
                    return;
                }
                let held = held & !bit(button);
                if let Some(at) = self.content_at(window, self.pointer) {
                    effects.push(Effect::Button {
                        window,
                        at,
                        button,
                        down,
                    });
                }
                self.pressed = (held != 0).then_some((window, held));
                if held == 0 {
                    self.hover_content(self.pointer, effects);
                }
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
        // A press of a button already held: its release was lost (to a card,
        // or never sent). The window hears it let go, and this press starts
        // over from what is under the pointer.
        if let Some((_, held)) = self.pressed
            && held & bit(button) != 0
        {
            self.cancel_press(effects);
        }
        // Another button while one is held in a window: that window's too.
        if let Some((window, held)) = self.pressed
            && let Some(at) = self.content_at(window, self.pointer)
        {
            self.pressed = Some((window, held | bit(button)));
            effects.push(Effect::Button {
                window,
                at,
                button,
                down,
            });
            return;
        }
        // Start takes every press while it is open: one inside is its own,
        // one outside closes it and goes nowhere else.
        if let Some(start) = self.start() {
            match start.contains(self.pointer) {
                true => effects.push(Effect::StartClick { at: self.pointer }),
                false => self.close_start(effects),
            }
            return;
        }
        let hit = self.hit(self.pointer);
        let Hit::Window(id, part) = hit else {
            match hit {
                Hit::Pane(PanePart::Close) if button == Button::Left => {
                    self.close_pane(effects);
                }
                Hit::Pane(part) => {
                    self.set_focus(Focus::Panel, effects);
                    let clicked = matches!(part, PanePart::Status | PanePart::Send);
                    if button == Button::Left && clicked {
                        effects.push(Effect::PanelClick { at: self.pointer });
                    }
                }
                hit @ (Hit::Taskbar(_) | Hit::Tray(_)) if button == Button::Left => {
                    self.bar_click(hit, effects);
                }
                // The empty desk takes the focus from every window, as a
                // click on the Windows desktop does: the keys go nowhere,
                // and a terminal's key lease ends.
                Hit::Desktop if button == Button::Left => {
                    self.set_focus(Focus::Nothing, effects);
                }
                _ => {}
            }
            return;
        };
        self.raise(id);
        self.set_focus(Focus::Window(id), effects);
        if button != Button::Left {
            if let Part::Content(at) = part {
                self.pressed = Some((id, bit(button)));
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
            Part::Content(at) => {
                self.pressed = Some((id, bit(button)));
                effects.push(Effect::Button {
                    window: id,
                    at,
                    button,
                    down,
                });
            }
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

    /// Ends a press held in a window: it hears each held button let go
    /// just outside its content, at (-1, -1), so it drops the press without
    /// taking it for a click on what is under the pointer.
    pub(super) fn cancel_press(&mut self, effects: &mut Vec<Effect>) {
        let Some((window, held)) = self.pressed.take() else {
            return;
        };
        if self.window(window).is_none() {
            return;
        }
        for button in [Button::Left, Button::Right, Button::Middle] {
            if held & bit(button) != 0 {
                effects.push(Effect::Button {
                    window,
                    at: Point { x: -1, y: -1 },
                    button,
                    down: false,
                });
            }
        }
    }

    /// `p` in `window`'s content's own px, wherever `p` is; None when the
    /// window is gone.
    fn content_at(&self, window: WindowId, p: Point) -> Option<Point> {
        let content = self.window(window)?.content();
        Some(Point {
            x: p.x - content.x,
            y: p.y - content.y,
        })
    }
}

/// `button`'s bit among those held.
fn bit(button: Button) -> u8 {
    match button {
        Button::Left => 1,
        Button::Right => 2,
        Button::Middle => 4,
    }
}
