//! The model manager's window on the host, through a real `uikit::Window`:
//! every state's tree is one `uikit` takes, a selection sends only what
//! changed, and each control gives the model's intent.

use comp_models::{Disk, Import, Intent, Models, Refusal, Sha256, Signed, Store, Stored};
use comp_models_ui::{
    Change, DETAIL, Entry, IMPORT, IMPORTING, MAX_CHARS, MAX_ROWS, SIZE, TABLE, USE, View, build,
    clip, diff, on_ui, status,
};
use uikit::a11y::A11yNode;
use uikit::layout::FixedAdvance;
use uikit::widget::{UiEvent, Value};
use uikit::window::Window;

const QWEN: Sha256 = [0xaa; 32];
const SMALL: Sha256 = [0xbb; 32];
const FIXTURE: Sha256 = [0xcc; 32];

fn signed(id: &str, name: &str, length: u64, sha256: Sha256) -> Signed {
    Signed {
        id: id.into(),
        name: name.into(),
        length,
        sha256,
        licence: "Apache-2.0".into(),
        source: format!("unsloth/{id}-GGUF@a1b2c3d"),
    }
}

/// Qwen3-4B runs; the role also signs a small model the store holds and a
/// fixture that is on an attached disk.
fn models() -> Models {
    Models {
        signed: vec![
            signed(
                "qwen3-4b-2507",
                "Qwen3-4B-Instruct-2507 Q4_K_M",
                2_497_280_256,
                QWEN,
            ),
            signed("qwen3-0.6b", "Qwen3-0.6B Q8_0", 639_446_688, SMALL),
            signed("fixture", "Fixture model", 4096, FIXTURE),
        ],
        store: Store {
            blobs: vec![
                Stored {
                    sha256: QWEN,
                    length: 2_497_280_256,
                },
                Stored {
                    sha256: SMALL,
                    length: 639_446_688,
                },
            ],
            running: Some(QWEN),
            ..Store::default()
        },
        disks: vec![
            Disk {
                id: 1,
                label: "kai-model".into(),
                length: 4096,
            },
            Disk {
                id: 2,
                label: "backup".into(),
                length: 10,
            },
        ],
        import: None,
    }
}

/// Puts a whole tree into a fresh window, as the component's first draw does,
/// and returns what the window's accessibility tree gives.
fn a11y(tree: &[Entry]) -> Vec<A11yNode> {
    let mut w = Window::new("Models", SIZE.0, SIZE.1);
    for e in tree {
        w.put(e.id, e.parent, e.layout, e.widget.clone())
            .unwrap_or_else(|err| panic!("{err:?} putting {e:?}"));
    }
    w.commit(&FixedAdvance).a11y
}

fn names(tree: &[Entry]) -> Vec<String> {
    a11y(tree).into_iter().map(|n| n.name).collect()
}

fn has(tree: &[Entry], id: u32) -> bool {
    tree.iter().any(|e| e.id == id)
}

#[test]
fn every_state_is_a_tree_uikit_takes() {
    let mut m = models();
    let imports = [
        None,
        Some(Import::Copying {
            disk: 1,
            done: 1024,
            length: 4096,
        }),
        Some(Import::Stored {
            disk: 1,
            sha256: FIXTURE,
        }),
        Some(Import::Refused {
            disk: 7,
            why: Refusal::NotSigned,
        }),
    ];
    for import in imports {
        m.import = import;
        for selected in [None, Some(QWEN), Some(SMALL), Some(FIXTURE), Some([0; 32])] {
            let tree = build(&m, &View { selected });
            let shown = names(&tree);
            assert!(shown.iter().any(|n| n == "Models"), "{shown:?}");
        }
    }
    let shown = a11y(&build(
        &m,
        &View {
            selected: Some(SMALL),
        },
    ));
    let table = shown.iter().find(|n| n.id == TABLE).unwrap();
    assert_eq!(
        table.value.as_deref(),
        Some("3 rows, selected \"Qwen3-0.6B Q8_0 | 609.8 MiB | Apache-2.0 | stored\"")
    );
    let labels: Vec<&str> = shown.iter().map(|n| n.name.as_str()).collect();
    assert!(labels.contains(&"Licence: Apache-2.0"), "{labels:?}");
    assert!(
        labels.contains(&"From unsloth/qwen3-0.6b-GGUF@a1b2c3d"),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"disk 7 was not imported: the signed models role names no model like it."),
        "{labels:?}"
    );
    m.disks.clear();
    let shown = names(&build(&m, &View::default()));
    assert!(
        shown.iter().any(|n| n.starts_with("Attach a disk")),
        "{shown:?}"
    );
}

#[test]
fn choosing_a_stored_model_gives_model_use() {
    let m = models();
    let mut view = View::default();
    assert_eq!(on_ui(&m, &mut view, &UiEvent::Pressed(USE)), (false, None));
    // The running model is first: nothing to switch to, so no button.
    let (redraw, _) = on_ui(&m, &mut view, &UiEvent::Changed(TABLE, Value::Index(0)));
    assert!(redraw);
    assert_eq!(view.selected, Some(QWEN));
    assert!(!has(&build(&m, &view), USE));
    assert_eq!(on_ui(&m, &mut view, &UiEvent::Pressed(USE)).1, None);
    on_ui(&m, &mut view, &UiEvent::Changed(TABLE, Value::Index(1)));
    assert_eq!(view.selected, Some(SMALL));
    assert!(has(&build(&m, &view), USE));
    assert_eq!(
        on_ui(&m, &mut view, &UiEvent::Pressed(USE)).1,
        Some(Intent::Use(SMALL))
    );
    // A signed model the store does not hold has no button either.
    on_ui(&m, &mut view, &UiEvent::Changed(TABLE, Value::Index(2)));
    assert_eq!(view.selected, Some(FIXTURE));
    assert!(!has(&build(&m, &view), USE));
    on_ui(&m, &mut view, &UiEvent::Changed(TABLE, Value::Index(9)));
    assert_eq!(view.selected, None);
}

#[test]
fn a_switch_and_its_undo_show_in_the_status_and_the_list() {
    let mut m = models();
    m.applied(&Intent::Use(SMALL));
    assert_eq!(
        status(&m),
        ["Qwen3-0.6B Q8_0 is used from the next restart."]
    );
    let shown = a11y(&build(&m, &View::default()));
    let table = shown.iter().find(|n| n.id == TABLE).unwrap();
    assert_eq!(table.value.as_deref(), Some("3 rows"));
    m.applied(&Intent::Use(QWEN));
    assert!(status(&m).is_empty());
    // After the restart: the new model on trial, then a rollback.
    m.store.running = Some(SMALL);
    m.store.on_trial = true;
    assert_eq!(
        status(&m),
        ["Qwen3-0.6B Q8_0 is on trial: it is kept once it loads and passes the smoke tasks."]
    );
    m.store.running = Some(QWEN);
    m.store.on_trial = false;
    m.store.rolled_back = Some((SMALL, "a smoke task failed".into()));
    assert_eq!(
        status(&m),
        [
            "Qwen3-0.6B Q8_0 did not pass its trial (a smoke task failed); back on Qwen3-4B-Instruct-2507 Q4_K_M."
        ]
    );
}

#[test]
fn import_is_offered_for_a_disk_of_a_signed_size_only() {
    let mut m = models();
    let mut view = View::default();
    let tree = build(&m, &view);
    assert!(has(&tree, IMPORT) && !has(&tree, IMPORT + 1));
    assert_eq!(
        on_ui(&m, &mut view, &UiEvent::Pressed(IMPORT)).1,
        Some(Intent::Import(1))
    );
    assert_eq!(on_ui(&m, &mut view, &UiEvent::Pressed(IMPORT + 1)).1, None);
    assert_eq!(on_ui(&m, &mut view, &UiEvent::Pressed(IMPORT + 9)).1, None);
    m.applied(&Intent::Import(1));
    m.progress(2048);
    let tree = build(&m, &view);
    assert!(!has(&tree, IMPORT), "one import at a time");
    let progress = a11y(&tree).into_iter().find(|n| n.id == IMPORTING).unwrap();
    assert_eq!(
        progress.name,
        "Importing from kai-model, checking its SHA-256"
    );
    m.imported(Ok(Stored {
        sha256: FIXTURE,
        length: 4096,
    }));
    let shown = names(&build(&m, &view));
    assert!(
        shown.contains(
            &"Imported Fixture model from kai-model: the signed models role names it.".into()
        ),
        "{shown:?}"
    );
}

#[test]
fn a_selection_sends_only_what_changed() {
    let m = models();
    let mut view = View::default();
    let first = build(&m, &view);
    on_ui(&m, &mut view, &UiEvent::Changed(TABLE, Value::Index(1)));
    let second = build(&m, &view);
    let changes = diff(&first, &second);
    assert!(
        !changes
            .iter()
            .any(|c| matches!(c, Change::Put(e) if e.id == comp_models_ui::HEADING))
    );
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, Change::Put(e) if e.id == DETAIL))
    );
    let mut w = Window::new("Models", SIZE.0, SIZE.1);
    for e in &first {
        w.put(e.id, e.parent, e.layout, e.widget.clone()).unwrap();
    }
    for c in changes {
        match c {
            Change::Put(e) => w.put(e.id, e.parent, e.layout, e.widget).unwrap(),
            Change::Remove(id) => w.remove(id).unwrap(),
        }
    }
    let applied: Vec<String> = w
        .commit(&FixedAdvance)
        .a11y
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(applied, names(&second));
}

#[test]
fn a_full_role_with_long_text_still_fits_the_window() {
    let long = "x".repeat(10_000);
    let n = MAX_ROWS + 5;
    let mut m = Models {
        signed: (0..n)
            .map(|i| {
                let mut sha = [0u8; 32];
                sha[..8].copy_from_slice(&(i as u64).to_le_bytes());
                let mut s = signed(&format!("m{i}"), &long, 1000 + i as u64, sha);
                s.licence = long.clone();
                s.source = long.clone();
                s
            })
            .collect(),
        disks: (0..n as u32)
            .map(|id| Disk {
                id,
                label: long.clone(),
                length: 1000 + u64::from(id),
            })
            .collect(),
        ..Models::default()
    };
    m.store.blobs.push(Stored {
        sha256: m.signed[0].sha256,
        length: 1000,
    });
    let selected = Some(m.signed[3].sha256);
    let tree = build(&m, &View { selected });
    let mut ids: Vec<u32> = tree.iter().map(|e| e.id).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "an id is used twice");
    for (i, e) in tree.iter().enumerate().skip(1) {
        assert!(
            tree[..i].iter().any(|p| p.id == e.parent),
            "{} before its parent {}",
            e.id,
            e.parent
        );
    }
    let shown = a11y(&tree);
    let table = shown.iter().find(|n| n.id == TABLE).unwrap();
    assert!(table.value.as_deref().unwrap().starts_with("256 rows"));
    assert_eq!(clip(&long).chars().count(), MAX_CHARS + 1);
    assert_eq!(clip("short"), "short");
}
