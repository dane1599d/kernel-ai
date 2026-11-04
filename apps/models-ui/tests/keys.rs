//! The Models window driven by keys through a real `uikit::Window`, as the OS
//! drives it (#556): a key press reaches the focused control, the control gives
//! the model's intent, and the window is drawn again. The `ui-models-use` boot
//! case sends the keys of [`the_boot_cases_two_stages_refresh_then_use`]
//! (QEMU's `tab`, `down`, `ret`), so a person at the keyboard switches the
//! model the way the case does.

use comp_models::{Intent, Models, Sha256, Signed, Store, Stored};
use comp_models_ui::{Change, Entry, SIZE, TABLE, USE, View, build, diff, on_ui};
use uikit::input::{Input, Key, Mods};
use uikit::layout::FixedAdvance;
use uikit::window::Window;

const FIXTURE: Sha256 = [0xcc; 32];
const FIXTURE_2: Sha256 = [0xdd; 32];

/// The keys that choose the stored model and press Use in a window no key has
/// reached yet, in QEMU's names, and the same as `uikit` keys: Tab past
/// Refresh to the table, Down twice to the second row (the first Down chooses
/// the first), Tab to Use, and Enter.
const BOOT_KEYS: [&str; 6] = ["tab", "tab", "down", "down", "tab", "ret"];
const KEYS: [Key; 6] = [
    Key::Tab,
    Key::Tab,
    Key::Down,
    Key::Down,
    Key::Tab,
    Key::Enter,
];

/// A window and the model behind it, as the component keeps them.
struct Keyed {
    models: Models,
    view: View,
    window: Window,
    tree: Vec<Entry>,
}

impl Keyed {
    fn new(models: Models) -> Self {
        // As the component opens it: it reads only, and its Use switches.
        let view = View {
            read_only: true,
            switches: true,
            ..View::default()
        };
        let tree = build(&models, &view);
        let mut window = Window::new("Models", SIZE.0, SIZE.1);
        for e in &tree {
            window
                .put(e.id, e.parent, e.layout, e.widget.clone())
                .unwrap();
        }
        window.commit(&FixedAdvance);
        Keyed {
            models,
            view,
            window,
            tree,
        }
    }

    fn redraw(&mut self) {
        let next = build(&self.models, &self.view);
        for change in diff(&self.tree, &next) {
            match change {
                Change::Put(e) => self.window.put(e.id, e.parent, e.layout, e.widget).unwrap(),
                Change::Remove(id) => self.window.remove(id).unwrap(),
            }
        }
        self.tree = next;
        self.window.commit(&FixedAdvance);
    }

    /// One key press; what the model is asked to do because of it.
    fn key(&mut self, key: Key) -> Vec<Intent> {
        let mut intents = Vec::new();
        let ev = Input::Key {
            key,
            down: true,
            mods: Mods::default(),
            text: String::new(),
        };
        for event in self.window.input(ev, &FixedAdvance) {
            let (draw, intent) = on_ui(&self.models, &mut self.view, &event);
            intents.extend(intent);
            if draw {
                self.redraw();
            }
        }
        intents
    }
}

fn signed(id: &str, name: &str, sha256: Sha256) -> Signed {
    Signed {
        id: id.into(),
        name: name.into(),
        length: 2_101_245,
        sha256,
        licence: "MIT".into(),
        source: String::new(),
    }
}

/// The list the boot case's window shows: fixture runs, and fixture-2, which
/// the case imported first, is stored.
fn models() -> Models {
    Models {
        signed: vec![
            signed("fixture", "Fixture model", FIXTURE),
            signed("fixture-2", "Second fixture model", FIXTURE_2),
        ],
        store: Store {
            blobs: [FIXTURE, FIXTURE_2]
                .map(|sha256| Stored {
                    sha256,
                    length: 2_101_245,
                })
                .to_vec(),
            running: Some(FIXTURE),
            ..Store::default()
        },
        ..Models::default()
    }
}

#[test]
fn the_boot_cases_keys_choose_the_stored_model_and_press_use() {
    let mut w = Keyed::new(models());
    let mut intents = Vec::new();
    for key in KEYS {
        intents.extend(w.key(key));
    }
    assert_eq!(w.view.selected, Some(FIXTURE_2));
    assert!(w.tree.iter().any(|e| e.id == USE));
    assert_eq!(intents, [Intent::Use(FIXTURE_2)]);
}

/// The `ui-models-use` boot case's two stages: the window first shows the list
/// it read before the case imported fixture-2, so `tab ret` presses Refresh;
/// once the list comes back with fixture-2 stored, `tab down down tab ret`
/// presses Use on it, with Refresh still focused.
#[test]
fn the_boot_cases_two_stages_refresh_then_use() {
    let mut before = models();
    before.store.blobs.retain(|b| b.sha256 == FIXTURE);
    let mut w = Keyed::new(before);
    assert!(w.key(Key::Tab).is_empty());
    assert!(w.key(Key::Enter).is_empty(), "Refresh is the component's");
    // The list comes back: fixture-2 is stored now.
    w.models = models();
    w.redraw();
    let mut intents = Vec::new();
    for key in [Key::Tab, Key::Down, Key::Down, Key::Tab, Key::Enter] {
        intents.extend(w.key(key));
    }
    assert_eq!(intents, [Intent::Use(FIXTURE_2)]);
}

#[test]
fn the_boot_keys_are_the_uikit_keys_by_name() {
    let named: Vec<&str> = KEYS
        .iter()
        .map(|k| match k {
            Key::Tab => "tab",
            Key::Down => "down",
            Key::Enter => "ret",
            other => panic!("{other:?} is not a key the case sends"),
        })
        .collect();
    assert_eq!(named, BOOT_KEYS);
}

#[test]
fn the_model_in_use_has_no_use_button() {
    let mut w = Keyed::new(models());
    // Tab to the table and Down once: the first row is fixture, which runs.
    w.key(Key::Tab);
    w.key(Key::Tab);
    w.key(Key::Down);
    assert_eq!(w.view.selected, Some(FIXTURE));
    assert!(!w.tree.iter().any(|e| e.id == USE));
    assert!(w.tree.iter().any(|e| e.id == TABLE));
}
