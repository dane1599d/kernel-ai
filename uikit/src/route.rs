//! Routing: the user's input becomes the same changes to the tree, and the same
//! events to the app, as the agent's actions (`act.rs`).

use crate::geom::{Point, Rect};
use crate::input::{Input, Key, Mods};
use crate::layout::Measure;
use crate::paint::{field_box, menu_item};
use crate::style::{TypeStyle, metric};
use crate::widget::{Axis, NodeId, ROOT, UiEvent, Value, Widget};
use crate::window::Window;
use alloc::string::String;
use alloc::vec::Vec;

impl Window {
    /// The deepest shown node under `p`.
    fn hit(&self, p: Point) -> Option<NodeId> {
        self.tree.preorder().into_iter().rev().find(|id| {
            self.places
                .get(id)
                .is_some_and(|pl| pl.rect.intersect(&pl.clip).contains(p))
        })
    }

    /// `id` or its nearest ancestor for which `want` holds.
    fn up(&self, id: NodeId, want: impl Fn(&Widget) -> bool) -> Option<NodeId> {
        let mut n = id;
        loop {
            let node = self.tree.get(n)?;
            if want(&node.widget) {
                return Some(n);
            }
            if n == ROOT {
                return None;
            }
            n = node.parent;
        }
    }

    fn rect(&self, id: NodeId) -> Option<Rect> {
        self.places.get(&id).map(|p| p.rect)
    }

    /// Focusable shown nodes, in tab order.
    fn focus_ring(&self) -> Vec<NodeId> {
        self.tree
            .preorder()
            .into_iter()
            .filter(|id| self.places.contains_key(id))
            .filter(|id| self.tree.get(*id).is_some_and(|n| n.widget.focusable()))
            .collect()
    }

    pub(crate) fn set_focus(&mut self, id: Option<NodeId>) {
        if self.st.focus != id {
            self.st.focus = id;
            self.dirty = true;
        }
        if let Some(id) = id {
            self.scroll_into_view(id);
        }
    }

    /// Scrolls every scroll view above `id` so that `id` shows.
    fn scroll_into_view(&mut self, id: NodeId) {
        let Some(target) = self.rect(id) else {
            return;
        };
        let mut n = self.tree.get(id).map(|n| n.parent);
        while let Some(p) = n {
            if let (Some(Widget::Scroll { axis }), Some(view)) =
                (self.tree.get(p).map(|n| &n.widget), self.rect(p))
            {
                let (start, len, vstart, vlen) = match axis {
                    Axis::Row => (target.x, target.w, view.x, view.w),
                    Axis::Column => (target.y, target.h, view.y, view.h),
                };
                let off = self.st.scroll.get(&p).copied().unwrap_or(0);
                let new = if start < vstart {
                    off - (vstart - start)
                } else if start + len > vstart + vlen {
                    off + (start + len - vstart - vlen).min(start - vstart)
                } else {
                    off
                };
                if new != off {
                    self.st.scroll.insert(p, new.max(0));
                    self.dirty = true;
                }
            }
            if p == ROOT {
                break;
            }
            n = self.tree.get(p).map(|n| n.parent);
        }
    }

    /// Handles one input event; what the app hears.
    pub fn input(&mut self, ev: Input, m: &dyn Measure) -> Vec<UiEvent> {
        self.relayout(m);
        let mut out = Vec::new();
        match ev {
            Input::FocusIn | Input::FocusOut => {
                self.st.window_focused = ev == Input::FocusIn;
                self.dirty = true;
            }
            Input::PointerMove(p) => self.pointer_move(p, m, &mut out),
            Input::Button { at, down: true } => self.press(at, m, &mut out),
            Input::Button { at, down: false } => {
                if let Some(b) = self.st.pressed.take() {
                    self.dirty = true;
                    let on = self
                        .hit(at)
                        .and_then(|h| self.up(h, |w| matches!(w, Widget::Button { .. })));
                    if on == Some(b)
                        && matches!(
                            self.tree.get(b).map(|n| &n.widget),
                            Some(Widget::Button { .. })
                        )
                    {
                        out.push(UiEvent::Pressed(b));
                    }
                }
            }
            Input::Wheel { at, dy } => {
                let scroll = self
                    .hit(at)
                    .and_then(|h| self.up(h, |w| matches!(w, Widget::Scroll { .. })));
                if let Some(s) = scroll {
                    let extent = self.places.get(&s).map_or(0, |p| p.extent);
                    let view = self.rect(s).map_or(0, |r| r.h.max(r.w));
                    let off = self.st.scroll.get(&s).copied().unwrap_or(0);
                    let new = (off + dy * metric::WHEEL_STEP).clamp(0, (extent - view).max(0));
                    if new != off {
                        self.st.scroll.insert(s, new);
                        self.dirty = true;
                    }
                }
            }
            Input::Key {
                key,
                down: true,
                mods,
                text,
            } => self.key(key, mods, &text, &mut out),
            Input::Key { down: false, .. } => {}
        }
        out
    }

    fn pointer_move(&mut self, p: Point, m: &dyn Measure, out: &mut Vec<UiEvent>) {
        if let Some(menu) = self.st.menu {
            let items = match self.tree.get(menu).map(|n| &n.widget) {
                Some(Widget::Menu { items, .. }) => items.len(),
                _ => 0,
            };
            let r = self.rect(menu).unwrap_or_default();
            let over = (0..items).find(|&i| menu_item(r, i).contains(p));
            if over != self.st.menu_hover {
                self.st.menu_hover = over;
                self.dirty = true;
            }
        }
        if let Some(s) = self.st.pressed.filter(|s| {
            matches!(
                self.tree.get(*s).map(|n| &n.widget),
                Some(Widget::Slider { .. })
            )
        }) {
            if let Some(v) = self.slider_at(s, p.x) {
                out.extend(self.change(s, Value::Number(v)).ok().flatten());
            }
            return;
        }
        let hover = self.hit(p).and_then(|h| {
            self.up(h, |w| {
                matches!(w, Widget::Button { .. } | Widget::Menu { .. })
            })
        });
        if hover != self.st.hover {
            self.st.hover = hover;
            self.dirty = true;
        }
        let _ = m;
    }

    /// The slider value under `x`.
    fn slider_at(&self, id: NodeId, x: i32) -> Option<i32> {
        let r = self.rect(id)?;
        let Some(Widget::Slider { min, max, step, .. }) = self.tree.get(id).map(|n| &n.widget)
        else {
            return None;
        };
        let w = i64::from(metric::SLIDER_W - metric::KNOB).max(1);
        let dx = i64::from(x - r.x - metric::KNOB / 2).clamp(0, w);
        let span = i64::from(*max) - i64::from(*min);
        let raw = i64::from(*min) + span.max(0) * dx / w;
        let step = i64::from((*step).max(1));
        let snapped = i64::from(*min) + (raw - i64::from(*min) + step / 2) / step * step;
        Some(snapped.clamp(i64::from(*min), i64::from(*max)) as i32)
    }

    /// The byte offset in `text` nearest `x` px from its start.
    fn caret_at(m: &dyn Measure, text: &str, x: i32) -> usize {
        let mut best = (i32::MAX, 0);
        for (i, _) in text
            .char_indices()
            .chain(core::iter::once((text.len(), ' ')))
        {
            let d = (m.advance(TypeStyle::Body, &text[..i]) - x).abs();
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }

    fn press(&mut self, at: Point, m: &dyn Measure, out: &mut Vec<UiEvent>) {
        if let Some(menu) = self.st.menu.take() {
            self.st.menu_hover = None;
            self.dirty = true;
            let r = self.rect(menu).unwrap_or_default();
            let n = match self.tree.get(menu).map(|n| &n.widget) {
                Some(Widget::Menu { items, .. }) => items.len(),
                _ => 0,
            };
            if let Some(i) = (0..n).find(|&i| menu_item(r, i).contains(at)) {
                out.extend(self.change(menu, Value::Index(i as u32)).ok().flatten());
                return;
            }
            if r.contains(at) {
                return;
            }
        }
        let Some(hit) = self.hit(at) else {
            return;
        };
        let target = self.up(hit, Widget::focusable);
        self.set_focus(target);
        let Some(t) = target else {
            return;
        };
        let r = self.rect(t).unwrap_or_default();
        let Some(w) = self.tree.get(t).map(|n| n.widget.clone()) else {
            return;
        };
        match w {
            Widget::Button { .. } => {
                self.st.pressed = Some(t);
                self.dirty = true;
            }
            Widget::Toggle { on, .. } => out.extend(self.change(t, Value::On(!on)).ok().flatten()),
            Widget::TextLine(f) | Widget::TextArea(f) => {
                let area = matches!(
                    self.tree.get(t).map(|n| &n.widget),
                    Some(Widget::TextArea(_))
                );
                let b = field_box(m, &f, r, area);
                let x = at.x - b.x - metric::CONTROL_PAD;
                let line_no = if area {
                    ((at.y - b.y - metric::AREA_PAD) / m.line(TypeStyle::Body)).max(0) as usize
                } else {
                    0
                };
                let mut start = 0;
                let mut caret = f.value.len();
                for (i, line) in f.value.split('\n').enumerate() {
                    if i == line_no {
                        caret = start + Self::caret_at(m, line, x);
                        break;
                    }
                    start += line.len() + 1;
                }
                self.st.caret.insert(t, caret);
                self.dirty = true;
            }
            Widget::ItemList { .. } => {
                let kids = self
                    .tree
                    .get(t)
                    .map(|n| n.children.clone())
                    .unwrap_or_default();
                if let Some(i) = kids
                    .iter()
                    .position(|k| self.rect(*k).is_some_and(|r| r.contains(at)))
                {
                    out.extend(self.change(t, Value::Index(i as u32)).ok().flatten());
                }
            }
            Widget::Table { .. } => {
                let row = (at.y - r.y - 1) / metric::ROW_H - 1;
                if row >= 0 {
                    out.extend(self.change(t, Value::Index(row as u32)).ok().flatten());
                }
            }
            Widget::Tabs { labels, .. } => {
                if at.y < r.y + metric::TAB_H {
                    let mut x = r.x;
                    for (i, w) in crate::layout::tab_widths(m, &labels)
                        .into_iter()
                        .enumerate()
                    {
                        if at.x >= x && at.x < x + w {
                            out.extend(self.change(t, Value::Index(i as u32)).ok().flatten());
                        }
                        x += w;
                    }
                }
            }
            Widget::Slider { .. } => {
                self.st.pressed = Some(t);
                if let Some(v) = self.slider_at(t, at.x) {
                    out.extend(self.change(t, Value::Number(v)).ok().flatten());
                }
            }
            Widget::Menu { .. } => {
                self.st.menu = Some(t);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn key(&mut self, key: Key, mods: Mods, text: &str, out: &mut Vec<UiEvent>) {
        if key == Key::Tab && !mods.command() {
            let ring = self.focus_ring();
            if ring.is_empty() {
                return;
            }
            let at = self
                .st
                .focus
                .and_then(|f| ring.iter().position(|&n| n == f));
            let next = match (at, mods.shift()) {
                (None, false) => 0,
                (None, true) => ring.len() - 1,
                (Some(i), false) => (i + 1) % ring.len(),
                (Some(i), true) => (i + ring.len() - 1) % ring.len(),
            };
            self.set_focus(Some(ring[next]));
            return;
        }
        if key == Key::Escape && self.st.menu.take().is_some() {
            self.st.menu_hover = None;
            self.dirty = true;
            return;
        }
        let Some(f) = self.st.focus else {
            return;
        };
        let Some(w) = self.tree.get(f).map(|n| n.widget.clone()) else {
            return;
        };
        let activate = matches!(key, Key::Space | Key::Enter);
        let step = |back: bool| if back { -1 } else { 1 };
        match w {
            Widget::Button { .. } if activate => out.push(UiEvent::Pressed(f)),
            Widget::Toggle { on, .. } if activate => {
                out.extend(self.change(f, Value::On(!on)).ok().flatten())
            }
            Widget::Menu { .. } if activate => {
                self.st.menu = Some(f);
                self.dirty = true;
            }
            Widget::TextLine(field) | Widget::TextArea(field) => {
                let area = matches!(
                    self.tree.get(f).map(|n| &n.widget),
                    Some(Widget::TextArea(_))
                );
                if key == Key::Enter && !area {
                    out.push(UiEvent::Submitted(f));
                    return;
                }
                out.extend(self.edit(f, &field.value, key, mods, text, area));
            }
            Widget::ItemList { selected, .. } | Widget::Table { selected, .. } => {
                let n = match &w {
                    Widget::Table { rows, .. } => rows.len(),
                    _ => self.tree.get(f).map_or(0, |n| n.children.len()),
                };
                let back = match key {
                    Key::Up => true,
                    Key::Down => false,
                    _ => return,
                };
                if n == 0 {
                    return;
                }
                let cur = selected.map_or(if back { n as i64 } else { -1 }, i64::from);
                let i = (cur + step(back)).clamp(0, n as i64 - 1) as u32;
                out.extend(self.change(f, Value::Index(i)).ok().flatten());
                if let Some(Widget::ItemList { .. }) = self.tree.get(f).map(|n| &n.widget)
                    && let Some(&child) = self.tree.get(f).and_then(|n| n.children.get(i as usize))
                {
                    self.scroll_into_view(child);
                }
            }
            Widget::Tabs { labels, selected } => {
                let back = match key {
                    Key::Left => true,
                    Key::Right => false,
                    _ => return,
                };
                let i = (i64::from(selected) + step(back))
                    .clamp(0, labels.len() as i64 - 1)
                    .max(0) as u32;
                out.extend(self.change(f, Value::Index(i)).ok().flatten());
            }
            Widget::Slider {
                min,
                max,
                step: s,
                value,
                ..
            } => {
                let s = s.max(1);
                let v = match key {
                    Key::Left | Key::Down => value.saturating_sub(s),
                    Key::Right | Key::Up => value.saturating_add(s),
                    Key::Home => min,
                    Key::End => max,
                    _ => return,
                };
                out.extend(self.change(f, Value::Number(v)).ok().flatten());
            }
            _ => {}
        }
    }

    /// One key in a text field: the caret moves or the text changes.
    fn edit(
        &mut self,
        f: NodeId,
        value: &str,
        key: Key,
        mods: Mods,
        text: &str,
        area: bool,
    ) -> Option<UiEvent> {
        let at = self
            .st
            .caret
            .get(&f)
            .copied()
            .unwrap_or(value.len())
            .min(value.len());
        let prev = value[..at].char_indices().last().map_or(0, |(i, _)| i);
        let next = value[at..].chars().next().map_or(at, |c| at + c.len_utf8());
        let line_start = value[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = value[at..].find('\n').map_or(value.len(), |i| at + i);
        let (caret, new): (usize, Option<String>) = match key {
            Key::Left => (prev, None),
            Key::Right => (next, None),
            Key::Home => (line_start, None),
            Key::End => (line_end, None),
            Key::Backspace if at > 0 => (prev, Some([&value[..prev], &value[at..]].concat())),
            Key::Delete if at < value.len() => (at, Some([&value[..at], &value[next..]].concat())),
            Key::Enter if area => (at + 1, Some([&value[..at], "\n", &value[at..]].concat())),
            _ if !mods.command() && !text.is_empty() && !text.chars().any(char::is_control) => (
                at + text.len(),
                Some([&value[..at], text, &value[at..]].concat()),
            ),
            _ => return None,
        };
        let ev = match new {
            Some(t) => self.change(f, Value::Text(t)).ok().flatten(),
            None => None,
        };
        if ev.is_some() || new_caret_differs(self.st.caret.get(&f), caret) {
            self.st.caret.insert(f, caret);
            self.dirty = true;
        }
        ev
    }
}

fn new_caret_differs(old: Option<&usize>, new: usize) -> bool {
    old != Some(&new)
}
