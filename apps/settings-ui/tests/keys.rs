//! The Settings window driven by keys through a real `uikit::Window`, as the OS
//! drives it: a key press reaches the focused control, the control gives the
//! model's intent, and the page is drawn again. The `app-settings` boot case
//! sends the same keys (QEMU's `tab`, `ret`, `right`, `spc`), so a person at the
//! keyboard changes a setting the way the case does.

use comp_settings::files::Appearance;
use comp_settings::{Intent, Page, Settings};
use comp_settings_ui::{
    Change, DEFAULT_WINDOW, Entry, SIZE, View, WINDOW_ON, build, diff, on_ui, redraw,
};
use uikit::input::{Input, Key, Mods};
use uikit::layout::FixedAdvance;
use uikit::window::Window;

/// A window and the model behind it, as the component keeps them.
struct Keyed {
    settings: Settings,
    view: View,
    window: Window,
    tree: Vec<Entry>,
}

impl Keyed {
    fn new(settings: Settings) -> Self {
        let mut view = View::default();
        let tree = build(&settings, &mut view);
        let mut window = Window::new("Settings", SIZE.0, SIZE.1);
        for e in &tree {
            window
                .put(e.id, e.parent, e.layout, e.widget.clone())
                .unwrap();
        }
        window.commit(&FixedAdvance);
        Keyed {
            settings,
            view,
            window,
            tree,
        }
    }

    /// One key press and release; what the model is asked to do because of it.
    fn key(&mut self, key: Key) -> Vec<Intent> {
        let mut intents = Vec::new();
        let ev = Input::Key {
            key,
            down: true,
            mods: Mods::default(),
            text: String::new(),
        };
        for event in self.window.input(ev, &FixedAdvance) {
            let (draw, intent) = on_ui(&self.settings, &mut self.view, &event);
            intents.extend(intent);
            if draw {
                let next = build(&self.settings, &mut self.view);
                for change in redraw(&self.tree, &next, &mut self.view) {
                    match change {
                        Change::Put(e) => {
                            self.window.put(e.id, e.parent, e.layout, e.widget).unwrap()
                        }
                        Change::Remove(id) => self.window.remove(id).unwrap(),
                    }
                }
                self.tree = next;
                self.window.commit(&FixedAdvance);
            }
        }
        // Nothing but the keys drew it, so the diff to a fresh build is empty.
        assert!(diff(&self.tree, &build(&self.settings, &mut self.view)).is_empty());
        intents
    }
}

fn settings() -> Settings {
    Settings {
        appearance: Appearance::default(),
        ..Settings::default()
    }
}

#[test]
fn tab_then_tab_then_enter_presses_the_theme_button() {
    let mut w = Keyed::new(settings());
    assert!(w.key(Key::Tab).is_empty());
    assert!(w.key(Key::Tab).is_empty());
    let intents = w.key(Key::Enter);
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert!(
        matches!(&intents[0], Intent::Appearance(a) if a.theme == look::THEMES[1].name),
        "{intents:?}"
    );
}

#[test]
fn the_restart_window_is_set_from_its_page_by_keys() {
    let mut w = Keyed::new(settings());
    // Tab to the page tabs, then Right to the Updates page.
    w.key(Key::Tab);
    let at = Page::ALL.iter().position(|p| *p == Page::Updates).unwrap();
    for _ in 0..at {
        assert!(w.key(Key::Right).is_empty());
    }
    assert_eq!(w.view.page, at);
    assert!(w.tree.iter().any(|e| e.id == WINDOW_ON));
    // Tab to the switch and press it.
    w.key(Key::Tab);
    let intents = w.key(Key::Space);
    assert_eq!(
        intents,
        [Intent::RestartWindow(Some(DEFAULT_WINDOW))],
        "{intents:?}"
    );
}

/// The two key sequences the `app-settings` boot case sends, one after the
/// other in the same window: the theme, then back round to the tabs for the
/// restart window. QEMU names them `tab tab ret` and `tab tab tab right right
/// right right tab spc`.
#[test]
fn the_boot_cases_two_key_sequences_in_one_session() {
    let mut w = Keyed::new(settings());
    let mut sent = Vec::new();
    for key in [Key::Tab, Key::Tab, Key::Enter] {
        sent.extend(w.key(key));
    }
    // The model has not taken the theme (the OS has not answered), so the
    // window is still on Appearance, with the Theme button focused.
    assert_eq!(w.view.page, 0);
    for key in [Key::Tab, Key::Tab, Key::Tab] {
        sent.extend(w.key(key));
    }
    for key in [Key::Right, Key::Right, Key::Right, Key::Right] {
        sent.extend(w.key(key));
    }
    for key in [Key::Tab, Key::Space] {
        sent.extend(w.key(key));
    }
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(matches!(&sent[0], Intent::Appearance(_)), "{sent:?}");
    assert_eq!(sent[1], Intent::RestartWindow(Some(DEFAULT_WINDOW)));
}
