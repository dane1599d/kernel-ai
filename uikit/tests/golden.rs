//! Golden display lists and accessibility trees, drawn with `FixedAdvance`. Set
//! `UIKIT_BLESS=1` to rewrite them after a deliberate change, and review the diff:
//! it is the look, in tokens and logical px.

mod common;

use std::fmt::Write as _;
use std::path::PathBuf;
use uikit::input::Input;
use uikit::layout::FixedAdvance;
use uikit::{Frame, Window};

fn golden(name: &str, text: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.txt"));
    if std::env::var_os("UIKIT_BLESS").is_some() {
        std::fs::write(&path, text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; run with UIKIT_BLESS=1", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(text, want, "{name} differs from {}", path.display());
}

fn a11y_text(f: &Frame) -> String {
    let mut out = String::new();
    for n in &f.a11y {
        let b = n.bounds;
        write!(out, "[{}] {} {:?}", n.id, n.role.name(), n.name).unwrap();
        if let Some(v) = &n.value {
            write!(out, " = {v:?}").unwrap();
        }
        write!(out, " at {},{} {}x{}", b.x, b.y, b.w, b.h).unwrap();
        let acts: Vec<_> = uikit::a11y::Action::ALL
            .iter()
            .filter(|a| n.allows(**a))
            .map(|a| a.name())
            .collect();
        if !acts.is_empty() {
            write!(out, " acts {}", acts.join(",")).unwrap();
        }
        if let Some(b) = &n.bound {
            write!(out, " runs {b}").unwrap();
        }
        if n.focused {
            out.push_str(" focused");
        }
        if !n.children.is_empty() {
            write!(out, " kids {:?}", n.children).unwrap();
        }
        out.push('\n');
    }
    out
}

fn frame(w: &mut Window) -> Frame {
    w.commit(&FixedAdvance)
}

#[test]
fn notes_window() {
    let mut w = common::notes();
    w.input(Input::FocusIn, &FixedAdvance);
    w.focus(6).unwrap();
    let f = frame(&mut w);
    golden("notes", &f.list.to_string());
    golden("notes-a11y", &a11y_text(&f));
}

#[test]
fn gallery_window() {
    let mut w = common::gallery();
    let f = frame(&mut w);
    golden("gallery", &f.list.to_string());
    golden("gallery-a11y", &a11y_text(&f));
}

#[test]
fn an_open_menu_draws_above_everything() {
    let mut w = common::gallery();
    frame(&mut w);
    w.act(6, uikit::a11y::Action::Focus, None, &FixedAdvance)
        .unwrap();
    w.input(
        uikit::input::Input::Key {
            key: uikit::input::Key::Space,
            down: true,
            mods: Default::default(),
            text: String::from(" "),
        },
        &FixedAdvance,
    );
    let f = frame(&mut w);
    let last = f.list.groups.last().unwrap();
    assert_eq!(last.node, 6);
    let mut text = String::new();
    for op in &last.ops {
        writeln!(text, "{op:?}").unwrap();
    }
    golden("menu-open", &text);
}

#[test]
fn apps_get_no_ai_or_ask_colour() {
    for mut w in [common::notes(), common::gallery()] {
        w.input(Input::FocusIn, &FixedAdvance);
        let f = frame(&mut w);
        let text = f.list.to_string();
        assert!(!text.contains(" ai ") && !text.contains(" ask "), "{text}");
    }
}
