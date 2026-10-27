//! The display list: what each shown node draws, by token and type style, in
//! logical px. comphost draws it through `gfx2d`; uikit never touches pixels.

use crate::geom::Rect;
use crate::layout::{self, Measure, Places, field_label_h};
use crate::state::State;
use crate::style::{Token, TypeStyle, metric::*};
use crate::tree::Tree;
use crate::widget::{NodeId, ROOT, TextField, Widget};
use alloc::string::String;
use alloc::vec::Vec;

pub use crate::list::{DisplayList, Group, Op};

/// A field's input box inside its node's rectangle, below its label.
pub fn field_box(m: &dyn Measure, f: &TextField, r: Rect, area: bool) -> Rect {
    let top = field_label_h(m, f);
    let h = if area {
        r.h - top
    } else {
        CONTROL_H.min(r.h - top)
    };
    Rect::new(r.x, r.y + top, r.w, h)
}

/// The rectangle of item `i` of an open menu below `r`.
pub fn menu_item(r: Rect, i: usize) -> Rect {
    Rect::new(r.x, r.y + r.h + 2 + i as i32 * ROW_H, r.w.max(160), ROW_H)
}

/// Text centred vertically in `r`, `PAD` from its left.
fn text_in(m: &dyn Measure, r: Rect, style: TypeStyle, token: Token, text: &str) -> Op {
    Op::Text {
        x: r.x + CONTROL_PAD,
        y: r.y + (r.h - m.line(style)) / 2,
        style,
        token,
        text: String::from(text),
    }
}

fn text_at(x: i32, y: i32, style: TypeStyle, token: Token, text: &str) -> Op {
    Op::Text {
        x,
        y,
        style,
        token,
        text: String::from(text),
    }
}

fn fill(rect: Rect, token: Token, radius: i32) -> Op {
    Op::Fill {
        rect,
        token,
        radius,
    }
}

fn frame(rect: Rect, token: Token) -> Op {
    Op::Stroke {
        rect,
        token,
        width: 1,
        radius: RADIUS,
    }
}

/// The caret at byte `at` of the line `line`, starting at `x`, top `y`.
fn caret(m: &dyn Measure, x: i32, y: i32, line: &str, at: usize) -> Op {
    let at = at.min(line.len());
    let at = (0..=at)
        .rev()
        .find(|&i| line.is_char_boundary(i))
        .unwrap_or(0);
    let dx = m.advance(TypeStyle::Body, &line[..at]);
    fill(
        Rect::new(x + dx, y + 2, 1, m.line(TypeStyle::Body) - 4),
        Token::Text,
        0,
    )
}

/// What `id`, at `r`, draws.
fn ops_of(
    tree: &Tree,
    places: &Places,
    st: &State,
    m: &dyn Measure,
    id: NodeId,
    r: Rect,
) -> Vec<Op> {
    let Some(node) = tree.get(id) else {
        return Vec::new();
    };
    let focused = st.focus == Some(id);
    let mut ops = Vec::new();
    match &node.widget {
        Widget::Stack { .. } if id == ROOT => ops.push(fill(r, Token::Window, 0)),
        Widget::Stack { .. } => {}
        Widget::Label { text, style, tone } => {
            for (i, line) in text.split('\n').enumerate() {
                ops.push(text_at(
                    r.x,
                    r.y + i as i32 * m.line(*style),
                    *style,
                    tone.token(),
                    line,
                ));
            }
        }
        Widget::Button { text, .. } => {
            let lit = st.pressed == Some(id) || st.hover == Some(id);
            ops.push(fill(
                r,
                if lit { Token::Line } else { Token::Raised },
                RADIUS,
            ));
            ops.push(frame(r, Token::LineStrong));
            let x = r.x + (r.w - m.advance(TypeStyle::Label, text)) / 2;
            ops.push(text_at(
                x,
                r.y + (r.h - m.line(TypeStyle::Label)) / 2,
                TypeStyle::Label,
                Token::Text,
                text,
            ));
        }
        Widget::TextLine(f) | Widget::TextArea(f) => {
            let area = matches!(node.widget, Widget::TextArea(_));
            if !f.label.is_empty() {
                ops.push(text_at(r.x, r.y, TypeStyle::Small, Token::Muted, &f.label));
            }
            let b = field_box(m, f, r, area);
            ops.push(fill(b, Token::Desk, RADIUS));
            ops.push(frame(b, Token::LineStrong));
            ops.push(Op::PushClip(b.inset(1)));
            let x = b.x + CONTROL_PAD;
            let y0 = if area {
                b.y + AREA_PAD
            } else {
                b.y + (b.h - m.line(TypeStyle::Body)) / 2
            };
            if f.value.is_empty() {
                ops.push(text_at(x, y0, TypeStyle::Body, Token::Dim, &f.placeholder));
            }
            let at = st.caret.get(&id).copied().unwrap_or(f.value.len());
            let mut start = 0;
            for (i, line) in f.value.split('\n').enumerate() {
                let y = y0 + i as i32 * m.line(TypeStyle::Body);
                if !line.is_empty() {
                    ops.push(text_at(x, y, TypeStyle::Body, Token::Text, line));
                }
                if focused && at >= start && at <= start + line.len() {
                    ops.push(caret(m, x, y, line, at - start));
                }
                start += line.len() + 1;
            }
            ops.push(Op::PopClip);
        }
        Widget::ItemList { selected, .. } => {
            ops.push(frame(r, Token::LineStrong));
            let item = selected
                .and_then(|s| node.children.get(s as usize))
                .and_then(|c| places.get(c));
            if let Some(p) = item {
                ops.push(fill(p.rect, Token::Raised, 0));
                ops.push(fill(
                    Rect::new(p.rect.x, p.rect.y, 2, p.rect.h),
                    Token::Accent,
                    0,
                ));
            }
        }
        Widget::Table {
            columns,
            rows,
            selected,
            ..
        } => {
            ops.push(frame(r, Token::LineStrong));
            let widths = layout::column_widths(m, columns, rows);
            let inner = r.inset(1);
            if let Some(s) = selected.filter(|&s| (s as usize) < rows.len()) {
                let y = inner.y + (s as i32 + 1) * ROW_H;
                ops.push(fill(
                    Rect::new(inner.x, y, inner.w, ROW_H),
                    Token::Raised,
                    0,
                ));
            }
            let mut x = inner.x;
            for (i, w) in widths.iter().enumerate() {
                if let Some(c) = columns.get(i) {
                    ops.push(text_in(
                        m,
                        Rect::new(x, inner.y, *w, ROW_H),
                        TypeStyle::Label,
                        Token::Muted,
                        c,
                    ));
                }
                for (j, row) in rows.iter().enumerate() {
                    if let Some(c) = row.get(i) {
                        let cell = Rect::new(x, inner.y + (j as i32 + 1) * ROW_H, *w, ROW_H);
                        ops.push(text_in(m, cell, TypeStyle::Body, Token::Text, c));
                    }
                }
                x += w;
            }
            ops.push(fill(
                Rect::new(inner.x, inner.y + ROW_H - 1, inner.w, 1),
                Token::Line,
                0,
            ));
        }
        Widget::Tabs { labels, selected } => {
            let mut x = r.x;
            for (i, (l, w)) in labels.iter().zip(layout::tab_widths(m, labels)).enumerate() {
                let tab = Rect::new(x, r.y, w, TAB_H);
                let on = i as u32 == *selected;
                let token = if on { Token::Text } else { Token::Muted };
                ops.push(Op::Text {
                    x: x + TAB_PAD,
                    y: r.y + (TAB_H - m.line(TypeStyle::Label)) / 2,
                    style: TypeStyle::Label,
                    token,
                    text: l.clone(),
                });
                if on {
                    ops.push(fill(
                        Rect::new(tab.x, tab.bottom() - 2, tab.w, 2),
                        Token::Accent,
                        0,
                    ));
                }
                x += w;
            }
            ops.push(fill(
                Rect::new(r.x, r.y + TAB_H - 1, r.w, 1),
                Token::Line,
                0,
            ));
        }
        Widget::Toggle { label, on } => {
            let track = Rect::new(r.x, r.y + (r.h - TOGGLE_H) / 2, TOGGLE_W, TOGGLE_H);
            ops.push(fill(
                track,
                if *on {
                    Token::Accent
                } else {
                    Token::LineStrong
                },
                TOGGLE_H / 2,
            ));
            let kx = if *on {
                track.right() - 2 - KNOB
            } else {
                track.x + 2
            };
            ops.push(fill(
                Rect::new(kx, track.y + 2, KNOB, KNOB),
                Token::Text,
                KNOB / 2,
            ));
            let ly = r.y + (r.h - m.line(TypeStyle::Body)) / 2;
            ops.push(text_at(
                track.right() + INLINE_GAP,
                ly,
                TypeStyle::Body,
                Token::Text,
                label,
            ));
        }
        Widget::Slider {
            label,
            min,
            max,
            value,
            ..
        } => {
            let track = Rect::new(
                r.x + KNOB / 2,
                r.y + (r.h - TRACK_H) / 2,
                SLIDER_W - KNOB,
                TRACK_H,
            );
            let span = i64::from(*max) - i64::from(*min);
            let at = i64::from((*value).clamp(*min, (*max).max(*min))) - i64::from(*min);
            let dx = if span > 0 {
                (i64::from(track.w) * at / span) as i32
            } else {
                0
            };
            ops.push(fill(track, Token::LineStrong, TRACK_H / 2));
            ops.push(fill(
                Rect::new(track.x, track.y, dx, TRACK_H),
                Token::Accent,
                TRACK_H / 2,
            ));
            let knob = Rect::new(track.x + dx - KNOB / 2, r.y + (r.h - KNOB) / 2, KNOB, KNOB);
            ops.push(fill(knob, Token::Text, KNOB / 2));
            let ly = r.y + (r.h - m.line(TypeStyle::Body)) / 2;
            ops.push(text_at(
                r.x + SLIDER_W + INLINE_GAP,
                ly,
                TypeStyle::Body,
                Token::Text,
                label,
            ));
        }
        Widget::Menu { label, .. } => {
            let lit = st.menu == Some(id) || st.hover == Some(id);
            ops.push(fill(
                r,
                if lit { Token::Line } else { Token::Raised },
                RADIUS,
            ));
            ops.push(frame(r, Token::LineStrong));
            ops.push(text_in(m, r, TypeStyle::Label, Token::Text, label));
            let icon = Rect::new(r.right() - CONTROL_PAD - 16, r.y + (r.h - 16) / 2, 16, 16);
            ops.push(Op::Icon {
                rect: icon,
                name: "chevron-down",
                token: Token::Muted,
            });
        }
        Widget::Scroll { axis } => {
            let p = places.get(&id).copied();
            let view = match axis {
                crate::widget::Axis::Row => r.w,
                crate::widget::Axis::Column => r.h,
            };
            if let Some(p) = p.filter(|p| p.extent > view && view > 0) {
                let off = st
                    .scroll
                    .get(&id)
                    .copied()
                    .unwrap_or(0)
                    .clamp(0, p.extent - view);
                let len = (view * view / p.extent).max(16);
                let pos = off * (view - len) / (p.extent - view);
                let thumb = match axis {
                    crate::widget::Axis::Row => {
                        Rect::new(r.x + pos, r.bottom() - SCROLLBAR_W, len, SCROLLBAR_W)
                    }
                    crate::widget::Axis::Column => {
                        Rect::new(r.right() - SCROLLBAR_W, r.y + pos, SCROLLBAR_W, len)
                    }
                };
                ops.push(fill(thumb, Token::LineStrong, SCROLLBAR_W / 2));
            }
        }
        Widget::Split { axis, .. } => {
            if let Some(p) = node.children.first().and_then(|c| places.get(c)) {
                let d = match axis {
                    crate::widget::Axis::Row => Rect::new(p.rect.right(), r.y, DIVIDER, r.h),
                    crate::widget::Axis::Column => Rect::new(r.x, p.rect.bottom(), r.w, DIVIDER),
                };
                ops.push(fill(d, Token::Line, 0));
            }
        }
        Widget::Image { source, .. } => ops.push(Op::Image {
            rect: r,
            source: source.clone(),
        }),
        Widget::Progress { label, done } => {
            let track = Rect::new(r.x, r.y + (r.h - PROGRESS_H) / 2, PROGRESS_W, PROGRESS_H);
            ops.push(fill(track, Token::Line, PROGRESS_H / 2));
            if let Some(d) = done {
                let w = track.w * i32::from((*d).min(1000)) / 1000;
                ops.push(fill(
                    Rect::new(track.x, track.y, w, PROGRESS_H),
                    Token::Accent,
                    PROGRESS_H / 2,
                ));
            }
            let ly = r.y + (r.h - m.line(TypeStyle::Body)) / 2;
            ops.push(text_at(
                track.right() + INLINE_GAP,
                ly,
                TypeStyle::Body,
                Token::Text,
                label,
            ));
        }
    }
    if focused && st.window_focused {
        let ring = focus_rect(m, &node.widget, r);
        let d = FOCUS_GAP + FOCUS_W;
        ops.push(Op::Stroke {
            rect: ring.outset(d),
            token: Token::Focus,
            width: FOCUS_W,
            radius: RADIUS + d,
        });
    }
    ops
}

/// What the focus ring goes around.
fn focus_rect(m: &dyn Measure, w: &Widget, r: Rect) -> Rect {
    match w {
        Widget::TextLine(f) => field_box(m, f, r, false),
        Widget::TextArea(f) => field_box(m, f, r, true),
        Widget::Tabs { .. } => Rect::new(r.x, r.y, r.w, TAB_H),
        _ => r,
    }
}

/// The bounding box of a node's ops.
fn bounds_of(ops: &[Op]) -> Rect {
    ops.iter().fold(Rect::default(), |acc, op| match op {
        Op::Fill { rect, .. } | Op::Icon { rect, .. } | Op::Image { rect, .. } => acc.union(rect),
        Op::Stroke { rect, width, .. } => acc.union(&rect.outset(*width)),
        Op::Text {
            x, y, style, text, ..
        } => {
            // Text bounds are approximate: wide enough for any font's glyphs.
            let w = style.size() * text.chars().count() as i32;
            acc.union(&Rect::new(*x, *y, w, style.line()))
        }
        Op::PushClip(_) | Op::PopClip => acc,
    })
}

/// The display list of the whole window.
pub fn paint(tree: &Tree, places: &Places, st: &State, m: &dyn Measure) -> DisplayList {
    let mut groups = Vec::new();
    let ring = FOCUS_GAP + FOCUS_W;
    let window = places.get(&ROOT).map_or(Rect::default(), |p| p.rect);
    for id in tree.preorder() {
        let Some(p) = places.get(&id) else {
            continue;
        };
        let ops = ops_of(tree, places, st, m, id, p.rect);
        if ops.is_empty() {
            continue;
        }
        let clip = if id == ROOT {
            p.clip
        } else {
            p.clip.outset(ring).intersect(&window)
        };
        let bounds = bounds_of(&ops).intersect(&clip);
        groups.push(Group {
            node: id,
            clip,
            bounds,
            ops,
        });
    }
    if let Some(menu) = st.menu
        && let (Some(Widget::Menu { items, .. }), Some(p)) =
            (tree.get(menu).map(|n| &n.widget), places.get(&menu))
    {
        let mut ops = Vec::new();
        let all = menu_item(p.rect, 0).union(&menu_item(p.rect, items.len().saturating_sub(1)));
        ops.push(fill(all.outset(1), Token::Raised, RADIUS));
        ops.push(frame(all.outset(1), Token::LineStrong));
        for (i, item) in items.iter().enumerate() {
            let r = menu_item(p.rect, i);
            if st.menu_hover == Some(i) {
                ops.push(fill(r, Token::Line, 0));
            }
            ops.push(text_in(m, r, TypeStyle::Label, Token::Text, item));
        }
        let bounds = bounds_of(&ops).intersect(&window);
        groups.push(Group {
            node: menu,
            clip: window,
            bounds,
            ops,
        });
    }
    DisplayList { groups }
}
