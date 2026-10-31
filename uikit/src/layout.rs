//! Layout: each node's natural size bottom-up, then its rectangle top-down. Both
//! passes walk the tree in a loop, never by recursion, since a tree may be 4,096
//! nodes deep and comphost's stack is small.

use crate::geom::Rect;
use crate::style::{TypeStyle, metric};
use crate::tree::Tree;
use crate::widget::{Axis, Insets, NodeId, ROOT, Size, TextField, Widget};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Text measurement: `gfx2d` provides the real one; tests use [`FixedAdvance`].
pub trait Measure {
    /// One line of `text`: its advance and its line height, in logical px. Pens
    /// are fractional; the scale and the legible setting are applied inside.
    fn measure(&self, style: TypeStyle, text: &str) -> (f32, f32);

    /// The advance, rounded up to whole logical px for layout.
    fn advance(&self, style: TypeStyle, text: &str) -> i32 {
        ceil(self.measure(style, text).0)
    }

    /// The line height, rounded up to whole logical px for layout.
    fn line(&self, style: TypeStyle) -> i32 {
        ceil(self.measure(style, "").1)
    }
}

/// `f32::ceil` without `std` or `libm`, for the sizes text has.
fn ceil(x: f32) -> i32 {
    let i = x as i32;
    if (i as f32) < x { i + 1 } else { i }
}

/// Every character three fifths of the style's size wide: a stand-in until
/// `gfx2d` lands, and what the golden display lists are drawn with.
pub struct FixedAdvance;

impl FixedAdvance {
    pub const fn char_w(style: TypeStyle) -> i32 {
        (style.size() * 3 + 2) / 5
    }
}

impl Measure for FixedAdvance {
    fn measure(&self, style: TypeStyle, text: &str) -> (f32, f32) {
        let w = Self::char_w(style) * text.chars().count() as i32;
        (w as f32, style.line() as f32)
    }
}

/// The widest line of `text`.
pub fn widest(m: &dyn Measure, style: TypeStyle, text: &str) -> i32 {
    text.split('\n')
        .map(|l| m.advance(style, l))
        .max()
        .unwrap_or(0)
}

/// Where a node landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Its rectangle, after any scrolling above it.
    pub rect: Rect,
    /// What of it may show.
    pub clip: Rect,
    /// For a scroll view: its child's length along the scroll axis.
    pub extent: i32,
}

/// Every shown node's place. Nodes not shown (pages of other tabs) are missing.
pub type Places = BTreeMap<NodeId, Placed>;

/// The space between a field's label and its box, if it has a label.
pub fn field_label_h(m: &dyn Measure, f: &TextField) -> i32 {
    if f.label.is_empty() {
        0
    } else {
        m.line(TypeStyle::Small) + metric::LABEL_GAP
    }
}

/// How many lines a text area shows.
pub fn area_lines(f: &TextField) -> i32 {
    (f.value.split('\n').count() as i32).max(metric::AREA_LINES)
}

/// The width of each table column.
pub fn column_widths(
    m: &dyn Measure,
    columns: &[alloc::string::String],
    rows: &[Vec<alloc::string::String>],
) -> Vec<i32> {
    let n = columns
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    (0..n)
        .map(|i| {
            let head = columns.get(i).map_or(0, |c| m.advance(TypeStyle::Label, c));
            let cells = rows
                .iter()
                .filter_map(|r| r.get(i))
                .map(|c| m.advance(TypeStyle::Body, c))
                .max()
                .unwrap_or(0);
            head.max(cells) + 2 * metric::CONTROL_PAD
        })
        .collect()
}

/// The width of each tab's label in the bar.
pub fn tab_widths(m: &dyn Measure, labels: &[alloc::string::String]) -> Vec<i32> {
    labels
        .iter()
        .map(|l| m.advance(TypeStyle::Label, l) + 2 * metric::TAB_PAD)
        .collect()
}

fn pad_w(p: &Insets) -> i32 {
    i32::from(p.left) + i32::from(p.right)
}

fn pad_h(p: &Insets) -> i32 {
    i32::from(p.top) + i32::from(p.bottom)
}

/// A leaf's content size, or a container's from its children's sizes.
fn natural_of(
    tree: &Tree,
    id: NodeId,
    m: &dyn Measure,
    nat: &BTreeMap<NodeId, (i32, i32)>,
) -> (i32, i32) {
    use metric::*;
    let Some(node) = tree.get(id) else {
        return (0, 0);
    };
    let kids: Vec<(i32, i32)> = node
        .children
        .iter()
        .map(|c| nat.get(c).copied().unwrap_or((0, 0)))
        .collect();
    let gap = i32::from(node.layout.gap);
    let along = |axis: Axis, extra: i32| {
        let gaps = gap * (kids.len() as i32 - 1).max(0);
        match axis {
            Axis::Row => (
                kids.iter().map(|k| k.0).sum::<i32>() + gaps + extra,
                kids.iter().map(|k| k.1).max().unwrap_or(0),
            ),
            Axis::Column => (
                kids.iter().map(|k| k.0).max().unwrap_or(0),
                kids.iter().map(|k| k.1).sum::<i32>() + gaps + extra,
            ),
        }
    };
    let (w, h) = match &node.widget {
        Widget::Stack { axis } => along(*axis, 0),
        Widget::Label { text, style, .. } => (
            widest(m, *style, text),
            m.line(*style) * text.split('\n').count() as i32,
        ),
        Widget::Button { text, .. } => (
            m.advance(TypeStyle::Label, text) + 2 * CONTROL_PAD,
            CONTROL_H,
        ),
        Widget::TextLine(f) => {
            let text = if f.value.is_empty() {
                &f.placeholder
            } else {
                &f.value
            };
            let w = (m.advance(TypeStyle::Body, text) + 2 * CONTROL_PAD).max(160);
            (w, CONTROL_H + field_label_h(m, f))
        }
        Widget::TextArea(f) => {
            let w = (widest(m, TypeStyle::Body, &f.value) + 2 * CONTROL_PAD).max(240);
            let h = area_lines(f) * m.line(TypeStyle::Body) + 2 * AREA_PAD;
            (w, h + field_label_h(m, f))
        }
        Widget::ItemList { .. } => {
            let w = kids.iter().map(|k| k.0).max().unwrap_or(0) + 2 * CONTROL_PAD;
            let h = kids.iter().map(|k| k.1.max(ROW_H)).sum::<i32>() + 2;
            (w, h.max(ROW_H + 2))
        }
        Widget::Table { columns, rows, .. } => (
            column_widths(m, columns, rows).iter().sum::<i32>() + 2,
            (rows.len() as i32 + 1) * ROW_H + 2,
        ),
        Widget::Tabs { labels, selected } => {
            let bar = tab_widths(m, labels).iter().sum::<i32>();
            let page = kids.get(*selected as usize).copied().unwrap_or((0, 0));
            (bar.max(page.0), TAB_H + page.1)
        }
        Widget::Toggle { label, .. } => (
            TOGGLE_W + INLINE_GAP + m.advance(TypeStyle::Body, label),
            CONTROL_H,
        ),
        Widget::Slider { label, .. } => (
            SLIDER_W + INLINE_GAP + m.advance(TypeStyle::Body, label),
            CONTROL_H,
        ),
        Widget::Menu { label, .. } => (
            m.advance(TypeStyle::Label, label) + 3 * CONTROL_PAD,
            CONTROL_H,
        ),
        Widget::Scroll { .. } => kids.first().copied().unwrap_or((0, 0)),
        Widget::Split { axis, .. } => along(*axis, DIVIDER),
        Widget::Image { .. } => (64, 64),
        Widget::Code(v) => crate::code::natural(m, v),
        Widget::Progress { label, .. } => (
            PROGRESS_W + INLINE_GAP + m.advance(TypeStyle::Body, label),
            CONTROL_H,
        ),
    };
    (
        w + pad_w(&node.layout.padding),
        h + pad_h(&node.layout.padding),
    )
}

/// Every node's natural size, children before parents.
fn naturals(tree: &Tree, m: &dyn Measure) -> BTreeMap<NodeId, (i32, i32)> {
    let mut nat = BTreeMap::new();
    for &id in tree.preorder().iter().rev() {
        let size = natural_of(tree, id, m, &nat);
        nat.insert(id, size);
    }
    nat
}

/// A child's length along one axis: fixed, natural or a share of `free`.
fn resolve(size: Size, natural: i32, available: i32) -> i32 {
    match size {
        Size::Fixed(px) => i32::from(px),
        Size::Fit => natural.min(available.max(0)),
        Size::Fill(_) => available.max(0),
    }
}

fn weight(size: Size) -> Option<i32> {
    match size {
        Size::Fill(w) => Some(i32::from(w.max(1))),
        _ => None,
    }
}

/// Splits `content` among `kids` along `axis`, Fill children sharing what is left.
fn run(
    tree: &Tree,
    kids: &[NodeId],
    nat: &BTreeMap<NodeId, (i32, i32)>,
    content: Rect,
    axis: Axis,
    gap: i32,
) -> Vec<(NodeId, Rect)> {
    let len = |r: Rect| if axis == Axis::Row { r.w } else { r.h };
    let main = |id: NodeId| {
        let n = tree.get(id).map(|n| n.layout);
        n.map_or(Size::Fit, |l| {
            if axis == Axis::Row { l.width } else { l.height }
        })
    };
    let cross = |id: NodeId| {
        let n = tree.get(id).map(|n| n.layout);
        n.map_or(Size::Fit, |l| {
            if axis == Axis::Row { l.height } else { l.width }
        })
    };
    let pick = |id: NodeId, row: bool| {
        let (w, h) = nat.get(&id).copied().unwrap_or((0, 0));
        if row { w } else { h }
    };
    let total = len(content) - gap * (kids.len() as i32 - 1).max(0);
    let fixed: i32 = kids
        .iter()
        .filter(|&&k| weight(main(k)).is_none())
        .map(|&k| resolve(main(k), pick(k, axis == Axis::Row), i32::MAX))
        .sum();
    let weights: i32 = kids.iter().filter_map(|&k| weight(main(k))).sum();
    let mut free = (total - fixed).max(0);
    let mut left_weight = weights;
    let mut pos = if axis == Axis::Row {
        content.x
    } else {
        content.y
    };
    let mut out = Vec::with_capacity(kids.len());
    for &k in kids {
        let l = match weight(main(k)) {
            Some(w) => {
                // The last Fill child takes the remainder, so rounding loses nothing.
                let share = if w == left_weight {
                    free
                } else {
                    free * w / left_weight
                };
                free -= share;
                left_weight -= w;
                share
            }
            None => resolve(main(k), pick(k, axis == Axis::Row), i32::MAX),
        };
        let r = match axis {
            Axis::Row => {
                let h = resolve(cross(k), pick(k, false), content.h).min(content.h);
                Rect::new(pos, content.y, l, h)
            }
            Axis::Column => {
                let w = resolve(cross(k), pick(k, true), content.w).min(content.w);
                Rect::new(content.x, pos, w, l)
            }
        };
        out.push((k, r));
        pos += l + gap;
    }
    out
}

/// Lays the tree out in a window of `size`, with scroll views at `scroll`.
pub fn layout(
    tree: &Tree,
    m: &dyn Measure,
    size: (i32, i32),
    scroll: &BTreeMap<NodeId, i32>,
) -> Places {
    let nat = naturals(tree, m);
    let window = Rect::new(0, 0, size.0, size.1);
    let mut places = Places::new();
    places.insert(
        ROOT,
        Placed {
            rect: window,
            clip: window,
            extent: 0,
        },
    );
    for id in tree.preorder() {
        let (Some(node), Some(&placed)) = (tree.get(id), places.get(&id)) else {
            continue;
        };
        let p = node.layout.padding;
        let content = Rect::new(
            placed.rect.x + i32::from(p.left),
            placed.rect.y + i32::from(p.top),
            placed.rect.w - pad_w(&p),
            placed.rect.h - pad_h(&p),
        );
        let gap = i32::from(node.layout.gap);
        let clip = placed.clip;
        let mut extent = 0;
        let kids: Vec<(NodeId, Rect, Rect)> = match &node.widget {
            Widget::Stack { axis } => run(tree, &node.children, &nat, content, *axis, gap)
                .into_iter()
                .map(|(k, r)| (k, r, clip))
                .collect(),
            Widget::ItemList { .. } => {
                let inner = content.inset(1);
                let mut y = inner.y;
                node.children
                    .iter()
                    .map(|&k| {
                        let h = nat.get(&k).map_or(0, |n| n.1).max(metric::ROW_H);
                        let pad = metric::ITEM_PAD;
                        let r = Rect::new(inner.x + pad, y, (inner.w - 2 * pad).max(0), h);
                        y += h;
                        (k, r, clip.intersect(&inner))
                    })
                    .collect()
            }
            Widget::Tabs { selected, .. } => node
                .children
                .get(*selected as usize)
                .map(|&k| {
                    let r = Rect::new(
                        content.x,
                        content.y + metric::TAB_H,
                        content.w,
                        content.h - metric::TAB_H,
                    );
                    (k, r, clip)
                })
                .into_iter()
                .collect(),
            Widget::Scroll { axis } => node
                .children
                .first()
                .map(|&k| {
                    let (nw, nh) = nat.get(&k).copied().unwrap_or((0, 0));
                    let viewport = clip.intersect(&content);
                    let (len, view) = match axis {
                        Axis::Row => (nw.max(content.w), content.w),
                        Axis::Column => (nh.max(content.h), content.h),
                    };
                    extent = len;
                    let off = scroll
                        .get(&id)
                        .copied()
                        .unwrap_or(0)
                        .clamp(0, (len - view).max(0));
                    let r = match axis {
                        Axis::Row => Rect::new(content.x - off, content.y, len, content.h),
                        Axis::Column => Rect::new(content.x, content.y - off, content.w, len),
                    };
                    (k, r, viewport)
                })
                .into_iter()
                .collect(),
            Widget::Split { axis, first } => {
                let along = match axis {
                    Axis::Row => content.w,
                    Axis::Column => content.h,
                } - metric::DIVIDER;
                let a = along.max(0) * i32::from((*first).min(1000)) / 1000;
                let b = along.max(0) - a;
                let rects = match axis {
                    Axis::Row => [
                        Rect::new(content.x, content.y, a, content.h),
                        Rect::new(content.x + a + metric::DIVIDER, content.y, b, content.h),
                    ],
                    Axis::Column => [
                        Rect::new(content.x, content.y, content.w, a),
                        Rect::new(content.x, content.y + a + metric::DIVIDER, content.w, b),
                    ],
                };
                node.children
                    .iter()
                    .zip(rects)
                    .map(|(&k, r)| (k, r, clip.intersect(&r)))
                    .collect()
            }
            _ => Vec::new(),
        };
        if extent != 0
            && let Some(pl) = places.get_mut(&id)
        {
            pl.extent = extent;
        }
        for (k, rect, kclip) in kids {
            places.insert(
                k,
                Placed {
                    rect,
                    clip: kclip,
                    extent: 0,
                },
            );
        }
    }
    places
}
