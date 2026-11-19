use super::*;

#[test]
fn the_versions_are_distinct_and_their_crates_exist() {
    let root = crate::workspace_root();
    let all: Vec<&Version> = VERSIONS.iter().chain(TEST_ONLY).collect();
    for (index, version) in all.iter().enumerate() {
        assert!(kabi::live::valid_label(version.label), "{}", version.label);
        let same = |v: &Version| {
            (v.recipe.component, v.label) == (version.recipe.component, version.label)
        };
        assert!(!all[..index].iter().any(|v| same(v)));
        assert!(version.recipe.wit.iter().all(|w| root.join(w).is_dir()));
        for dir in version.recipe.dirs {
            assert!(root.join(dir).join("Cargo.toml").exists(), "{dir}");
        }
    }
}

#[test]
fn the_source_bundle_holds_the_crate_and_its_component_dependencies() {
    let root = crate::workspace_root();
    let bundle = bundle_source(&root, &[BAD, NOTES]).unwrap();
    let files = artifact::source_files(&bundle).unwrap();
    let paths: Vec<&str> = files.iter().map(|(path, _)| *path).collect();
    assert!(paths.contains(&"components/notes/src/migrate.rs"));
    assert!(paths.contains(&"components/fixtures/notes-bad/src/lib.rs"));
    assert!(paths.contains(&"components/notes/Cargo.toml"));
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
}

fn fixture(component: &'static str) -> Version {
    Version {
        label: "1",
        recipe: Recipe {
            component,
            krate: "none",
            dirs: &[],
            feature: None,
            wit: &[],
        },
    }
}

#[test]
fn the_test_only_versions_are_not_in_the_set_that_ships() {
    for fixture in TEST_ONLY {
        assert!(
            !VERSIONS
                .iter()
                .any(|v| module_path(v) == module_path(fixture)),
            "{}",
            module_path(fixture)
        );
        assert!(kabi::live::valid_label(fixture.label));
        let root = crate::workspace_root();
        assert!(fixture.recipe.wit.iter().all(|w| root.join(w).is_dir()));
        for dir in fixture.recipe.dirs {
            assert!(root.join(dir).join("Cargo.toml").exists(), "{dir}");
        }
    }
}

#[test]
fn an_image_that_ships_refuses_a_fixture() {
    let module = |path: &str| Module {
        path: path.into(),
        bytes: Vec::new(),
    };
    let fixtures = [fixture("fixture-window")];
    let clean = [
        module("comp/notes/1.wasm"),
        module("comp/settings-window/1.wasm"),
    ];
    assert_eq!(check_none_of(&clean, &fixtures), Ok(()));
    let dirty = [
        module("comp/notes/1.wasm"),
        module("comp/fixture-window/1.wasm"),
    ];
    assert_eq!(module_path(&fixtures[0]), "comp/fixture-window/1.wasm");
    let why = check_none_of(&dirty, &fixtures).unwrap_err();
    assert!(why.contains("comp/fixture-window/1.wasm"), "{why}");
    // A component whose name only starts the same, or another version of the
    // same component, is not the fixture.
    let near = [
        module("comp/fixture-window-x/1.wasm"),
        module("comp/fixture-window/2.wasm"),
    ];
    assert_eq!(check_none_of(&near, &fixtures), Ok(()));
    // The list that ships is checked with `check_no_test_only`: `notes` 1
    // and 2 ship, its broken versions do not.
    assert_eq!(check_no_test_only(&clean), Ok(()));
    let shipped: Vec<Module> = VERSIONS.iter().map(|v| module(&module_path(v))).collect();
    assert_eq!(check_no_test_only(&shipped), Ok(()));
    for broken in ["3-import", "3-health", "3-trap", "3-spin"] {
        let path = std::format!("comp/notes/{broken}.wasm");
        let why = check_no_test_only(&[module("comp/notes/2.wasm"), module(&path)]);
        assert!(why.unwrap_err().contains(&path), "{broken}");
    }
}

/// The images that ship call the check before they are made, and the test
/// images are the only ones built with the fixtures.
#[test]
fn the_shipping_images_run_the_check_and_only_the_test_images_build_fixtures() {
    for (name, source) in [
        ("release_image.rs", include_str!("../release_image.rs")),
        ("vbox/mod.rs", include_str!("../vbox/mod.rs")),
    ] {
        assert!(source.contains("components::check_no_test_only("), "{name}");
        assert!(!source.contains("build_for_tests"), "{name}");
    }
    // The tasks that boot test cases (build, test, the smoke) and tune build
    // the test images, with the fixtures; none of them packs only what ships.
    for (name, source) in [
        ("main.rs", include_str!("../main.rs")),
        ("tune/boot.rs", include_str!("../tune/boot.rs")),
    ] {
        assert!(source.contains("components::build_for_tests("), "{name}");
        assert!(!source.contains("components::build("), "{name}");
    }
}

/// Notes' window build carries Notes' own action rows, under its world.
#[test]
fn the_window_build_packs_with_notes_actions() {
    let window = VERSIONS
        .iter()
        .find(|v| v.recipe.component == "notes-window")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("components/notes-window/src/lib.rs", b"//")]);
    let bytes = pack(&root, &window.recipe, window.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(parsed.manifest.world, "app:notes-window/notes-window@1.0.0");
    let actions = livecore::actions::parse(parsed.actions.unwrap()).unwrap();
    let add = actions.by_name("notes_add").unwrap();
    assert_eq!(add.export, "app:notes/actions@1.0.0#add");
    // The shell loads them for the `notes-window` service, which fronts
    // Notes' rows (docs/decisions.md, A-core's `<pkg>-window` rule).
    let answer = format!("version 1\n{}", parsed.actions.unwrap());
    let mut rows = shellcore::apps::AppRows::default();
    assert_eq!(rows.load("notes-window", &answer), Ok(actions.rows.len()));
    assert!(rows.find_in("notes-window", "notes_add").is_some());
}

/// The Files build carries its own action rows, under its world.
#[test]
fn the_files_build_packs_with_files_actions() {
    let files = VERSIONS
        .iter()
        .find(|v| v.recipe.component == "files-window")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("apps/files-window/src/lib.rs", b"//")]);
    let bytes = pack(&root, &files.recipe, files.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(parsed.manifest.world, "app:files-window/files-window@1.0.0");
    let actions = livecore::actions::parse(parsed.actions.unwrap()).unwrap();
    for (row, export) in [
        ("files_open", "open"),
        ("files_reveal", "reveal"),
        ("files_select", "select"),
    ] {
        let found = actions.by_name(row).unwrap();
        assert_eq!(
            found.export,
            std::format!("app:files/actions@1.0.0#{export}")
        );
    }
}

/// The Settings build carries its own action row, under its world.
#[test]
fn the_settings_build_packs_with_the_settings_action() {
    let settings = VERSIONS
        .iter()
        .find(|v| v.recipe.component == "settings-window")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("apps/settings-window/src/lib.rs", b"//")]);
    let bytes = pack(&root, &settings.recipe, settings.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(
        parsed.manifest.world,
        "app:settings-window/settings-window@1.0.0"
    );
    let actions = livecore::actions::parse(parsed.actions.unwrap()).unwrap();
    let open = actions.by_name("settings_open").unwrap();
    assert_eq!(open.export, "app:settings/actions@1.0.0#open");
}

/// The Settings fixture packs with a window and its one effect row.
/// The model manager's build packs under its world with no action rows: the
/// model rows are the OS's, and the window reads only.
#[test]
fn the_models_build_packs_with_no_action_rows() {
    let models = VERSIONS
        .iter()
        .find(|v| v.recipe.component == "models-window")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("apps/models-window/src/lib.rs", b"//")]);
    let bytes = pack(&root, &models.recipe, models.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(
        parsed.manifest.world,
        "app:models-window/models-window@1.0.0"
    );
    let actions = livecore::actions::parse(parsed.actions.unwrap()).unwrap();
    assert!(actions.rows.is_empty());
}

#[test]
fn the_settings_fixture_packs_with_its_one_effect_row() {
    let fixture = TEST_ONLY
        .iter()
        .find(|v| v.recipe.component == "fixture-window")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("apps/grant-fixture/src/lib.rs", b"//")]);
    let bytes = pack(&root, &fixture.recipe, fixture.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(
        parsed.manifest.world,
        "app:fixture-window/fixture-window@1.0.0"
    );
    assert!(parsed.actions.unwrap().contains("fs_list"));
}

/// `wait-reads`' fixture packs with its two effect rows and its one
/// action, under its own world.
#[test]
fn the_wait_fixture_packs_with_its_effects_and_actions() {
    let fixture = TEST_ONLY
        .iter()
        .find(|v| v.recipe.component == "wait-window")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("apps/wait-fixture/src/lib.rs", b"//")]);
    let bytes = pack(&root, &fixture.recipe, fixture.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(parsed.manifest.world, "app:wait-window/wait-window@1.0.0");
    let text = parsed.actions.unwrap();
    assert!(text.contains("effects fs_list fs_create"), "{text}");
    let actions = livecore::actions::parse(text).unwrap();
    assert_eq!(actions.rows.len(), 1, "{text}");
    // The shell loads them as comphost answers `kai-actions 1`, under the
    // running version, and its effects may call the two rows the world
    // declares.
    let mut rows = shellcore::apps::AppRows::default();
    let answer = format!(
        "version 1
{text}"
    );
    assert_eq!(rows.load("wait-window", &answer), Ok(1));
    assert!(rows.find_in("wait-window", "wait_seen").is_some());
    assert!(rows.may_effect("wait-window", "fs_list"));
    assert!(rows.may_effect("wait-window", "fs_create"));
    assert!(!rows.may_effect("wait-window", "fs_trash"));
}

/// The editor build carries its three action rows, under its world.
#[test]
fn the_editor_build_packs_with_the_editor_actions() {
    let editor = VERSIONS
        .iter()
        .find(|v| v.recipe.component == "editor")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("apps/editor-window/src/lib.rs", b"//")]);
    let bytes = pack(&root, &editor.recipe, editor.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(parsed.manifest.world, "app:editor/editor@1.0.0");
    let actions = livecore::actions::parse(parsed.actions.unwrap()).unwrap();
    let open = actions.by_name("editor_open").unwrap();
    assert_eq!(open.export, "app:editor/actions@1.0.0#open");
    assert!(actions.by_name("editor_status").is_some());
    // Opening a file by path is a read the request names: read-only, undoable
    // by nothing, and a path that must come from the request.
    let file = actions.by_name("editor_open_file").unwrap();
    assert_eq!(file.export, "app:editor/actions@1.0.0#open-file");
    assert_eq!(file.facts.read_only, Some(true));
    assert_eq!(file.facts.destructive, Some(false));
    assert_eq!(file.facts.open_world, Some(false));
    assert_eq!(file.args.len(), 1);
    assert_eq!(file.args[0].name, "path");
    assert_eq!(file.args[0].max, Some(255));
    assert!(file.args[0].from_request);
}

/// The Terminal build carries its three rows, under its world.
#[test]
fn the_terminal_build_packs_with_its_three_actions() {
    let terminal = VERSIONS
        .iter()
        .find(|v| v.recipe.component == "terminal-window")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("apps/terminal-window/src/lib.rs", b"//")]);
    let bytes = pack(&root, &terminal.recipe, terminal.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(
        parsed.manifest.world,
        "app:terminal-window/terminal-window@1.0.0"
    );
    let actions = livecore::actions::parse(parsed.actions.unwrap()).unwrap();
    for (name, export) in [
        ("terminal_window_run", "run"),
        ("terminal_window_read", "read"),
        ("terminal_window_clear", "clear"),
    ] {
        let row = actions.by_name(name).unwrap();
        assert_eq!(
            row.export,
            format!("app:terminal-window/actions@1.0.0#{export}")
        );
    }
}

/// The Monitor build carries its own action rows, under its world.
#[test]
fn the_monitor_build_packs_with_the_monitor_actions() {
    let monitor = VERSIONS
        .iter()
        .find(|v| v.recipe.component == "monitor-window")
        .unwrap();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let root = crate::workspace_root();
    let source = artifact::bundle(&[("apps/monitor-window/src/lib.rs", b"//")]);
    let bytes = pack(&root, &monitor.recipe, monitor.label, module, &source).unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(
        parsed.manifest.world,
        "app:monitor-window/monitor-window@1.0.0"
    );
    let actions = livecore::actions::parse(parsed.actions.unwrap()).unwrap();
    for (row, export) in [("monitor_sort", "sort"), ("monitor_select", "select")] {
        let found = actions.by_name(row).unwrap();
        assert_eq!(
            found.export,
            std::format!("app:monitor/actions@1.0.0#{export}")
        );
    }
}

#[test]
fn a_packed_module_keeps_its_bytes_and_says_what_it_is() {
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let source = artifact::bundle(&[("components/notes/src/lib.rs", b"//")]);
    let root = crate::workspace_root();
    let bytes = pack(
        &root,
        &TEST_ONLY[0].recipe,
        TEST_ONLY[0].label,
        module.clone(),
        &source,
    )
    .unwrap();
    let parsed = artifact::parse(&bytes).unwrap();
    assert_eq!(parsed.manifest.lifecycle, 2);
    let actions = livecore::actions::parse(parsed.actions.unwrap()).unwrap();
    assert!(actions.by_name("notes_add").is_some());
    assert!(bytes.starts_with(&module));
    assert_eq!(parsed.manifest.version, "3-import");
    assert_eq!(parsed.manifest.features, "bug-import");
    assert_eq!(parsed.manifest.build, sha256_hex(&module));
    assert_eq!(parsed.source, &source[..]);
}
