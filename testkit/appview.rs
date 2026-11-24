//! The agent's view of an app's real window (S05-2, ADR 0005 §6): the frame
//! the app's `uikit` window commits, made a tree by comphost's own
//! `wasmhost::surface::tree_update`, then projected as `ui_view` projects it
//! (`deskcore::view::project`). Not a crate: each app's `-ui` crate includes it
//! with `#[path = "../../../testkit/appview.rs"] mod appview;` (a path of the
//! right depth) in a test, with `deskcore`, `surfabi` and `wasmhost` among its
//! dev-dependencies. The committed `tests/golden/view.txt` is what `cargo xtask
//! tokens` counts in real tokens (xtask/src/tokens.rs).

#![allow(dead_code)]

use deskcore::desk::WindowId;
use deskcore::view::{self, Listed, MAX_TOKENS, View, estimate_tokens};
use surfabi::tree::Tree;

/// The view of `frame` as the focused window `app` (its `title`), with a
/// terminal beside it in the list, as `ui_view` answers.
pub fn project(frame: &uikit::Frame, app: &str, title: &str) -> View {
    let mut tree = Tree::default();
    tree.apply(wasmhost::surface::tree_update(frame))
        .expect("the frame is a tree the desk accepts");
    let listed = |id, app: &str, title: &str, focused| Listed {
        id: WindowId(id),
        app: app.into(),
        title: title.into(),
        focused,
    };
    let windows = [
        listed(1, app, title, true),
        listed(2, "terminal", "kai>", false),
    ];
    view::project(&windows, WindowId(1), &tree)
}

/// The view of `frame` is whole (not cut), within [`MAX_TOKENS`] by the host's
/// estimate, offers a control to act on, and is the committed
/// `tests/golden/view-<name>.txt`. `UIKIT_BLESS=1` rewrites the golden.
pub fn check(name: &str, app: &str, title: &str, frame: &uikit::Frame) -> View {
    let view = project(frame, app, title);
    let tokens = estimate_tokens(&view.text);
    assert!(
        tokens <= MAX_TOKENS,
        "{name}: {tokens} tokens\n{}",
        view.text
    );
    assert!(
        !view.text.contains(" more"),
        "{name} is cut:\n{}",
        view.text
    );
    // The window line is the first; each control the agent can act on after it
    // carries a short id in brackets.
    let controls: usize = view
        .text
        .lines()
        .skip(1)
        .map(|l| l.matches('[').count())
        .sum();
    assert!(
        controls >= 1,
        "{name}: {controls} short ids
{}",
        view.text
    );
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("view-{name}.txt"));
    let text = format!("{}\n", view.text);
    if std::env::var_os("UIKIT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return view;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; run with UIKIT_BLESS=1", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(text, want, "{name}'s view differs from {}", path.display());
    view
}

fn golden(name: &str, text: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.txt"));
    if std::env::var_os("UIKIT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; run with UIKIT_BLESS=1", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(text, want, "{name} differs from {}", path.display());
}

/// The frame's display list and its accessibility tree, as the committed
/// `tests/golden/<name>.txt` and `<name>-a11y.txt` (as the other apps' goldens).
pub fn goldens(name: &str, frame: &uikit::Frame) {
    golden(name, &frame.list.to_string());
    let tree: String = frame
        .a11y
        .iter()
        .map(|n| {
            let value = n
                .value
                .as_ref()
                .map_or(String::new(), |v| format!(" = {v:?}"));
            let runs = n
                .bound
                .as_ref()
                .map_or(String::new(), |b| format!(" runs {b}"));
            format!("[{}] {} {:?}{value}{runs}\n", n.id, n.role.name(), n.name)
        })
        .collect();
    golden(&format!("{name}-a11y"), &tree);
}

const INTER: &[u8] = include_bytes!("../assets/fonts/InterVariable.ttf");
const MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");

/// The frame painted by `uipaint` at 1x in each shipped theme (dark, light,
/// high-contrast and legible), each image's pixels hashed: the committed
/// `tests/golden/paint-<name>.sha256`, a line `<theme> <sha256>` each.
/// `UIKIT_BLESS=1` rewrites it. Needs `gfx2d`, `look`, `uipaint` and `sha2`
/// among the dev-dependencies.
pub fn paints(name: &str, frame: &uikit::Frame) {
    use sha2::{Digest, Sha256};
    let mut lines = String::new();
    for theme in look::theme::shipped() {
        let mut text = gfx2d::text::Text::new(
            gfx2d::text::Fonts {
                sans: INTER,
                mono: MONO,
            },
            theme.dark(),
        )
        .expect("the built-in fonts parse");
        text.set_theme(&theme);
        let palette = theme.palette();
        let size = uipaint::Scale::of(uipaint::SCALE_ONE).rect(uikit::geom::Rect::new(
            0,
            0,
            frame.size.0,
            frame.size.1,
        ));
        let mut img = gfx2d::Image::new(size.w, size.h);
        let mut painter = uipaint::Painter {
            palette: &palette,
            text: &mut text,
            scale: uipaint::Scale::of(uipaint::SCALE_ONE),
        };
        let all = img.bounds();
        painter.paint(&mut img.canvas(), &frame.list, all);
        let mut hash = Sha256::new();
        for p in &img.px {
            hash.update(p.to_le_bytes());
        }
        let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        lines.push_str(&format!("{} {hex}\n", theme.name));
    }
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("paint-{name}.sha256"));
    if std::env::var_os("UIKIT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &lines).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; run with UIKIT_BLESS=1", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(
        lines,
        want,
        "{name}'s painting differs from {}",
        path.display()
    );
}
