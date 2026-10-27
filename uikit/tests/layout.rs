//! Layout: shares, padding, tabs, scrolling and splits.

use std::collections::BTreeMap;
use uikit::geom::Rect;
use uikit::layout::{FixedAdvance, layout};
use uikit::style::Tone;
use uikit::style::TypeStyle;
use uikit::tree::Tree;
use uikit::widget::Layout;
use uikit::widget::{Axis, Insets, ROOT, Size, Widget};

fn label(text: &str) -> Widget {
    Widget::Label {
        text: String::from(text),
        style: TypeStyle::Body,
        tone: Tone::Normal,
    }
}

fn sized(w: Size, h: Size) -> Layout {
    Layout {
        width: w,
        height: h,
        ..Layout::FIT
    }
}

#[test]
fn fill_children_share_what_fixed_and_fit_leave() {
    let mut t = Tree::new();
    t.put(
        1,
        ROOT,
        Layout {
            gap: 10,
            ..Layout::FILL
        },
        Widget::Stack { axis: Axis::Row },
    )
    .unwrap();
    t.put(2, 1, sized(Size::Fixed(100), Size::Fill(1)), label("a"))
        .unwrap();
    t.put(3, 1, sized(Size::Fill(1), Size::Fit), label("b"))
        .unwrap();
    t.put(4, 1, sized(Size::Fill(2), Size::Fit), label("c"))
        .unwrap();
    t.put(5, 1, Layout::FIT, label("dd")).unwrap();
    let p = layout(&t, &FixedAdvance, (400, 300), &BTreeMap::new());
    // 400 - 3 gaps - 100 - 16 ("dd") = 254, shared 1:2.
    assert_eq!(p[&2].rect, Rect::new(0, 0, 100, 300));
    assert_eq!(p[&3].rect, Rect::new(110, 0, 84, 20));
    assert_eq!(p[&4].rect, Rect::new(204, 0, 170, 20));
    assert_eq!(p[&5].rect, Rect::new(384, 0, 16, 20));
}

#[test]
fn padding_and_column_gaps() {
    let mut t = Tree::new();
    let pad = Layout {
        padding: Insets {
            top: 8,
            right: 4,
            bottom: 8,
            left: 12,
        },
        gap: 6,
        ..Layout::FILL
    };
    t.put(1, ROOT, pad, Widget::Stack { axis: Axis::Column })
        .unwrap();
    t.put(2, 1, Layout::FIT, label("one")).unwrap();
    t.put(3, 1, Layout::FIT, label("two")).unwrap();
    let p = layout(&t, &FixedAdvance, (200, 100), &BTreeMap::new());
    assert_eq!(p[&2].rect, Rect::new(12, 8, 24, 20));
    assert_eq!(p[&3].rect, Rect::new(12, 34, 24, 20));
}

#[test]
fn only_the_selected_tab_is_placed() {
    let mut t = Tree::new();
    let labels = vec![String::from("A"), String::from("B")];
    t.put(
        1,
        ROOT,
        Layout::FILL,
        Widget::Tabs {
            labels,
            selected: 1,
        },
    )
    .unwrap();
    t.put(2, 1, Layout::FILL, label("page a")).unwrap();
    t.put(3, 1, Layout::FILL, label("page b")).unwrap();
    let p = layout(&t, &FixedAdvance, (300, 200), &BTreeMap::new());
    assert!(!p.contains_key(&2));
    assert_eq!(p[&3].rect, Rect::new(0, 32, 300, 168));
}

#[test]
fn scroll_offsets_are_clamped_and_clip_the_child() {
    let mut t = Tree::new();
    t.put(1, ROOT, Layout::FILL, Widget::Scroll { axis: Axis::Column })
        .unwrap();
    t.put(2, 1, Layout::FILL, Widget::Stack { axis: Axis::Column })
        .unwrap();
    for i in 0..10 {
        t.put(10 + i, 2, Layout::FIT, label("row")).unwrap();
    }
    let mut scroll = BTreeMap::new();
    scroll.insert(1, 1000);
    let p = layout(&t, &FixedAdvance, (100, 50), &scroll);
    assert_eq!(p[&1].extent, 200);
    assert_eq!(p[&2].rect, Rect::new(0, -150, 100, 200));
    assert_eq!(p[&2].clip, Rect::new(0, 0, 100, 50));
    assert_eq!(p[&19].rect.y, 30);
}

#[test]
fn split_divides_per_mille_around_the_divider() {
    let mut t = Tree::new();
    t.put(
        1,
        ROOT,
        Layout::FILL,
        Widget::Split {
            axis: Axis::Row,
            first: 250,
        },
    )
    .unwrap();
    t.put(2, 1, Layout::FILL, label("l")).unwrap();
    t.put(3, 1, Layout::FILL, label("r")).unwrap();
    let p = layout(&t, &FixedAdvance, (401, 100), &BTreeMap::new());
    assert_eq!(p[&2].rect, Rect::new(0, 0, 100, 100));
    assert_eq!(p[&3].rect, Rect::new(101, 0, 300, 100));
}

#[test]
fn a_deep_tree_lays_out_without_recursion() {
    let mut t = Tree::new();
    let mut parent = ROOT;
    for id in 1..4000 {
        t.put(
            id,
            parent,
            Layout::FIT,
            Widget::Stack { axis: Axis::Column },
        )
        .unwrap();
        parent = id;
    }
    let p = layout(&t, &FixedAdvance, (100, 100), &BTreeMap::new());
    assert_eq!(p.len(), 4000);
}

/// gfx2d's pens are fractional: layout rounds advances and lines up.
struct Quarter;

impl uikit::layout::Measure for Quarter {
    fn measure(&self, _: TypeStyle, text: &str) -> (f32, f32) {
        (text.len() as f32 * 7.25, 20.5)
    }
}

#[test]
fn fractional_measures_round_up_at_layout() {
    let mut t = Tree::new();
    t.put(1, ROOT, Layout::FIT, label("abcd")).unwrap();
    let p = layout(&t, &Quarter, (200, 100), &BTreeMap::new());
    assert_eq!(p[&1].rect, Rect::new(0, 0, 29, 21));
}
