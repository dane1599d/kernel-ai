//! Live component versions: each built for wasm32 with the `component` profile,
//! packed into an artifact (the module, its manifest, its action rows and its
//! source) and shipped
//! as boot module `comp/<component>/<label>.wasm`.

use crate::esp::Module;
use crate::sha256_hex;
use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION, SOURCE_SECTION};
use livecore::artifact::{self, Manifest};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

pub const TARGET: &str = "wasm32-unknown-unknown";

/// How a component version is built.
pub struct Recipe {
    pub component: &'static str,
    pub krate: &'static str,
    /// The crate's directory, and those of path dependencies whose source ships.
    pub dirs: &'static [&'static str],
    pub feature: Option<&'static str>,
    /// The app's WIT packages, the first with its world: the source of its
    /// action rows.
    pub wit: &'static [&'static str],
}

/// One version on the boot drive.
pub struct Version {
    pub label: &'static str,
    pub recipe: Recipe,
}

const NOTES: &str = "components/notes";
const BAD: &str = "components/fixtures/notes-bad";
/// Every notes version implements `app:notes/actions@1.0.0` from here.
const NOTES_WIT: &str = "components/notes/wit";
/// The in-tree packages an app's world may use.
const KAI_WIT: &[&str] = &["wit/kai-live", "wit/kai-app"];
const WINDOW: &str = "components/notes-window";
const NOTES_UI: &str = "components/notes-ui";
const WINDOW_WIT: &str = "components/notes-window/wit";
/// The Files app (P4 task 5): its window, model, tree and the shared `kai:ui` glue.
const FILES_WINDOW: &str = "apps/files-window";
const FILES_DIRS: &[&str] = &[FILES_WINDOW, "apps/files", "apps/files-ui", "apps/ui-bind"];
const FILES_WINDOW_WIT: &str = "apps/files-window/wit";
/// `app:files/actions`, which the window exports.
const FILES_WIT: &str = "apps/files/wit";
/// The Settings app (P4 task 7): its window, model, tree and the shared `kai:ui` glue.
const SETTINGS_WINDOW: &str = "apps/settings-window";
const SETTINGS_DIRS: &[&str] = &[
    SETTINGS_WINDOW,
    "apps/settings",
    "apps/settings-ui",
    "apps/ui-bind",
];
const SETTINGS_WINDOW_WIT: &str = "apps/settings-window/wit";
/// `app:settings/actions`, which the window exports.
const SETTINGS_WIT: &str = "apps/settings/wit";

/// The Terminal app (P4 task 4): its window, model, tree and the shared `kai:ui` glue.
const TERMINAL_WINDOW: &str = "apps/terminal-window";
const TERMINAL_DIRS: &[&str] = &[
    TERMINAL_WINDOW,
    "apps/terminal",
    "apps/terminal-ui",
    "apps/ui-bind",
];
const TERMINAL_WINDOW_WIT: &str = "apps/terminal-window/wit";
/// The System monitor (P4 task 9): its window, model, tree, the sample format it
/// reads (`systemabi`) and the shared `kai:ui` glue.
const MONITOR_WINDOW: &str = "apps/monitor-window";
const MONITOR_DIRS: &[&str] = &[
    MONITOR_WINDOW,
    "apps/monitor",
    "apps/monitor-ui",
    "apps/ui-bind",
    "systemabi",
];
const MONITOR_WINDOW_WIT: &str = "apps/monitor-window/wit";
/// `app:monitor/actions`, which the window exports.
const MONITOR_WIT: &str = "apps/monitor/wit";

/// `notes` from `components/notes`: version 2, and every upload the edit loop
/// builds.
pub const NOTES_RECIPE: Recipe = Recipe {
    component: "notes",
    krate: "comp-notes",
    dirs: &[NOTES],
    feature: None,
    wit: &[NOTES_WIT],
};

const fn bad(label: &'static str, feature: &'static str) -> Version {
    Version {
        label,
        recipe: Recipe {
            component: "notes",
            krate: "comp-notes-bad",
            dirs: &[BAD, NOTES],
            feature: Some(feature),
            wit: &[NOTES_WIT],
        },
    }
}

/// Every version on the boot drive: `notes` 1 and 2, and the broken ones the M9a
/// test expects to roll back.
pub const VERSIONS: &[Version] = &[
    Version {
        label: "1",
        recipe: Recipe {
            component: "notes",
            krate: "comp-notes-v1",
            dirs: &["components/fixtures/notes-v1"],
            feature: None,
            wit: &[NOTES_WIT],
        },
    },
    Version {
        label: "2",
        recipe: NOTES_RECIPE,
    },
    bad("3-import", "bug-import"),
    bad("3-health", "bug-health"),
    bad("3-trap", "bug-trap"),
    bad("3-spin", "bug-spin"),
    // Notes with its window (P3 task 9): the build P4 ships, its own component
    // so the edit loop's `notes` is untouched.
    Version {
        label: "1",
        recipe: Recipe {
            component: "notes-window",
            krate: "comp-notes-window",
            dirs: &[WINDOW, NOTES_UI, NOTES],
            feature: None,
            wit: &[WINDOW_WIT, "wit/kai-ui", NOTES_WIT],
        },
    },
    // The Files app (P4 task 5), a window over the OS's file rows.
    Version {
        label: "1",
        recipe: Recipe {
            component: "files-window",
            krate: "comp-files-window",
            dirs: FILES_DIRS,
            feature: None,
            wit: &[FILES_WINDOW_WIT, "wit/kai-ui", FILES_WIT],
        },
    },
    // The Terminal app (P4 task 4), today's shell in a window.
    Version {
        label: "1",
        recipe: Recipe {
            component: "terminal-window",
            krate: "comp-terminal-window",
            dirs: TERMINAL_DIRS,
            feature: None,
            wit: &[TERMINAL_WINDOW_WIT, "wit/kai-ui"],
        },
    },
    // The Settings app (P4 task 7), a window over the system's settings.
    Version {
        label: "1",
        recipe: Recipe {
            component: "settings-window",
            krate: "comp-settings-window",
            dirs: SETTINGS_DIRS,
            feature: None,
            wit: &[
                SETTINGS_WINDOW_WIT,
                "wit/kai-ui",
                "wit/kai-settings",
                SETTINGS_WIT,
            ],
        },
    },
    // The System monitor (P4 task 9), a window over a reading of the machine.
    Version {
        label: "1",
        recipe: Recipe {
            component: "monitor-window",
            krate: "comp-monitor-window",
            dirs: MONITOR_DIRS,
            feature: None,
            wit: &[
                MONITOR_WINDOW_WIT,
                "wit/kai-ui",
                "wit/kai-system",
                MONITOR_WIT,
            ],
        },
    },
];

/// Versions only the test images carry: a fixture that exists to be driven by a
/// boot case, never shipped. [`build`] leaves them out, so the release and slot
/// images, the VM image and the package repository never hold one;
/// [`build_for_tests`] adds them for the images `cargo xtask test` boots, and
/// [`check_no_test_only`] fails an image that has one. A version here is not also
/// in [`VERSIONS`].
pub const TEST_ONLY: &[Version] = &[
    // The `app-settings` case revokes and restores its one read-only `fs_list`
    // row in Settings, and presses its button between (P4 task 7).
    Version {
        label: "1",
        recipe: Recipe {
            component: "fixture-window",
            krate: "comp-grant-fixture",
            dirs: FIXTURE_DIRS,
            feature: None,
            wit: &["apps/grant-fixture/wit", "wit/kai-ui"],
        },
    },
];

/// The test fixtures' directories: the window, and the shared `kai:ui` glue.
const FIXTURE_DIRS: &[&str] = &["apps/grant-fixture", "apps/ui-bind"];

/// Builds and packs every version that ships.
pub fn build(root: &Path) -> Result<Vec<Module>, String> {
    check_target()?;
    VERSIONS
        .iter()
        .map(|version| build_one(root, version))
        .collect()
}

/// [`build`], and the [`TEST_ONLY`] fixtures too: for the images the boot cases
/// run, and nothing else.
pub fn build_for_tests(root: &Path) -> Result<Vec<Module>, String> {
    check_target()?;
    VERSIONS
        .iter()
        .chain(TEST_ONLY)
        .map(|version| build_one(root, version))
        .collect()
}

/// Fails if one of `modules` is a [`TEST_ONLY`] fixture: what an image that
/// ships (the release and slot images, a VM image) is checked with before it is
/// made.
pub fn check_no_test_only(modules: &[Module]) -> Result<(), String> {
    check_none_of(modules, TEST_ONLY)
}

/// Where a version's module sits on the boot drive.
fn module_path(version: &Version) -> String {
    format!("comp/{}/{}.wasm", version.recipe.component, version.label)
}

/// By exact module path: a component may ship some versions and keep others for
/// the tests (notes ships 1 and 2 and tests 3-import), so the component's name
/// alone is not the fixture.
fn check_none_of(modules: &[Module], fixtures: &[Version]) -> Result<(), String> {
    for fixture in fixtures {
        let path = module_path(fixture);
        if modules.iter().any(|m| m.path == path) {
            return Err(format!(
                "{path} is a test fixture and must not ship in this image"
            ));
        }
    }
    Ok(())
}

fn check_target() -> Result<(), String> {
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let output = Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .map_err(|e| format!("cannot run rustc: {e}"))?;
    let sysroot = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if !sysroot.join("lib").join("rustlib").join(TARGET).exists() {
        return Err(format!(
            "the {TARGET} target is not installed; run: rustup target add {TARGET}"
        ));
    }
    Ok(())
}

fn build_one(root: &Path, version: &Version) -> Result<Module, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args([
            "build",
            "--quiet",
            "-p",
            version.recipe.krate,
            "--target",
            TARGET,
            "--profile",
            "component",
        ])
        .env("KAI_VERSION", version.label);
    if let Some(feature) = version.recipe.feature {
        command.args(["--features", feature]);
    }
    let status = command
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!(
            "building {} {} failed",
            version.recipe.component, version.label
        ));
    }
    let wasm = root
        .join("target")
        .join(TARGET)
        .join("component")
        .join(format!("{}.wasm", version.recipe.krate.replace('-', "_")));
    let module = fs::read(&wasm).map_err(|e| format!("cannot read {}: {e}", wasm.display()))?;
    let source = bundle_source(root, version.recipe.dirs)?;
    let (module_len, source_len) = (module.len(), source.len());
    let bytes = pack(root, &version.recipe, version.label, module, &source)?;
    artifact::parse(&bytes)
        .map_err(|why| format!("{} {}: {why}", version.recipe.component, version.label))?;
    println!(
        "xtask: {} {} {} KiB ({} KiB module, {} KiB source)",
        version.recipe.component,
        version.label,
        bytes.len() / 1024,
        module_len / 1024,
        source_len / 1024
    );
    Ok(Module {
        path: module_path(version),
        bytes,
    })
}

fn world(recipe: &Recipe) -> String {
    format!("app:{0}/{0}@1.0.0", recipe.component)
}

/// The app's `kai-actions 1` section, from its WIT under `root`.
pub fn actions(root: &Path, recipe: &Recipe) -> Result<String, String> {
    let dirs: Vec<PathBuf> = KAI_WIT
        .iter()
        .chain(recipe.wit)
        .map(|dir| root.join(dir))
        .collect();
    crate::kai_actions::write(&dirs, &world(recipe))
}

/// The module with its manifest, action rows and source sections appended. The
/// rows come from the app's WIT under `root`.
pub fn pack(
    root: &Path,
    recipe: &Recipe,
    label: &str,
    mut module: Vec<u8>,
    source: &[u8],
) -> Result<Vec<u8>, String> {
    let world = world(recipe);
    let actions = actions(root, recipe)?;
    let manifest = Manifest {
        lifecycle: 2,
        component: recipe.component.into(),
        version: label.into(),
        world,
        engine: "wasmi".into(),
        features: recipe.feature.unwrap_or("-").into(),
        // The workspace licence (Cargo.toml), which every in-tree component uses.
        licence: "MPL-2.0".into(),
        build: sha256_hex(&module),
        source: sha256_hex(source),
    };
    artifact::append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
    artifact::append_custom_section(&mut module, ACTIONS_SECTION, actions.as_bytes());
    artifact::append_custom_section(&mut module, SOURCE_SECTION, source);
    Ok(module)
}

/// Cargo.toml and everything under src/ in each directory, by workspace path.
pub fn bundle_source(root: &Path, dirs: &[&str]) -> Result<Vec<u8>, String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for dir in dirs {
        let manifest = format!("{dir}/Cargo.toml");
        files.push((manifest.clone(), read(&root.join(&manifest))?));
        collect(root, &root.join(dir).join("src"), &mut files)?;
    }
    files.sort();
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    Ok(artifact::bundle(&borrowed))
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect(root, &path, files)?;
        } else {
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
            let name = relative.to_string_lossy().replace('\\', "/");
            files.push((name, read(&path)?));
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_versions_are_distinct_and_their_crates_exist() {
        let root = crate::workspace_root();
        for (index, version) in VERSIONS.iter().enumerate() {
            assert!(kabi::live::valid_label(version.label), "{}", version.label);
            let same = |v: &Version| {
                (v.recipe.component, v.label) == (version.recipe.component, version.label)
            };
            assert!(!VERSIONS[..index].iter().any(same));
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
                    .any(|v| v.recipe.component == fixture.recipe.component),
                "{}",
                fixture.recipe.component
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
        // The list that ships is checked with `check_no_test_only`.
        assert_eq!(check_no_test_only(&clean), Ok(()));
    }

    /// The images that ship call the check before they are made, and the test
    /// images are the only ones built with the fixtures.
    #[test]
    fn the_shipping_images_run_the_check_and_only_the_test_images_build_fixtures() {
        for (name, source) in [
            ("release_image.rs", include_str!("release_image.rs")),
            ("vbox/mod.rs", include_str!("vbox/mod.rs")),
        ] {
            assert!(source.contains("components::check_no_test_only("), "{name}");
            assert!(!source.contains("build_for_tests"), "{name}");
        }
        // The tasks that boot test cases (build, test, the smoke) and tune build
        // the test images, with the fixtures; none of them packs only what ships.
        for (name, source) in [
            ("main.rs", include_str!("main.rs")),
            ("tune/boot.rs", include_str!("tune/boot.rs")),
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
            &VERSIONS[2].recipe,
            VERSIONS[2].label,
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
}
