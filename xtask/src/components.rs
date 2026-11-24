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
/// The editor (P4 task 11): its window, the model's tree and the shared glue.
const EDITOR_WINDOW: &str = "apps/editor-window";
const EDITOR_DIRS: &[&str] = &[EDITOR_WINDOW, "apps/editor-ui", "apps/ui-bind"];
/// `app:editor/actions`, which the window exports.
const EDITOR_WIT: &str = "apps/editor/wit";

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
/// The model manager (P4 task 10): its window, list, tree, the list format it
/// reads (`modelchan`) and the shared `kai:ui` glue. It exports no actions.
const MODELS_WINDOW: &str = "apps/models-window";
const MODELS_DIRS: &[&str] = &[
    MODELS_WINDOW,
    "apps/models",
    "apps/models-ui",
    "apps/ui-bind",
    "modelchan",
];
const MODELS_WINDOW_WIT: &str = "apps/models-window/wit";

/// `notes` from `components/notes`: version 2, and every upload the edit loop
/// builds.
pub const NOTES_RECIPE: Recipe = Recipe {
    component: "notes",
    krate: "comp-notes",
    dirs: &[NOTES],
    feature: None,
    wit: &[NOTES_WIT],
};

/// Notes with its window (P3 task 9): the build P4 ships, its own component so
/// the edit loop's `notes` is untouched. Package `notes` carries it.
pub const NOTES_WINDOW_RECIPE: Recipe = Recipe {
    component: "notes-window",
    krate: "comp-notes-window",
    dirs: &[WINDOW, NOTES_UI, NOTES],
    feature: None,
    wit: &[WINDOW_WIT, "wit/kai-ui", NOTES_WIT],
};

/// The Files app (P4 task 5), a window over the OS's file rows.
pub const FILES_WINDOW_RECIPE: Recipe = Recipe {
    component: "files-window",
    krate: "comp-files-window",
    dirs: FILES_DIRS,
    feature: None,
    wit: &[FILES_WINDOW_WIT, "wit/kai-ui", FILES_WIT],
};

/// The Terminal app (P4 task 4), today's shell in a window.
pub const TERMINAL_WINDOW_RECIPE: Recipe = Recipe {
    component: "terminal-window",
    krate: "comp-terminal-window",
    dirs: TERMINAL_DIRS,
    feature: None,
    wit: &[TERMINAL_WINDOW_WIT, "wit/kai-ui"],
};

/// The Settings app (P4 task 7), a window over the system's settings.
pub const SETTINGS_WINDOW_RECIPE: Recipe = Recipe {
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
};

/// The System monitor (P4 task 9), a window over a reading of the machine.
pub const MONITOR_WINDOW_RECIPE: Recipe = Recipe {
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
};

/// The model manager (P4 task 10), a window over the model list that reads
/// only: a switch or an import is Kai's row.
pub const MODELS_WINDOW_RECIPE: Recipe = Recipe {
    component: "models-window",
    krate: "comp-models-window",
    dirs: MODELS_DIRS,
    feature: None,
    wit: &[MODELS_WINDOW_WIT, "wit/kai-ui", "wit/kai-models"],
};

/// The editor (P4 task 11), a window over the edit service and the person's
/// files. Its component is `editor`, not `editor-window`: the kernel grants
/// `edit` to that name alone.
pub const EDITOR_RECIPE: Recipe = Recipe {
    component: "editor",
    krate: "comp-editor-window",
    dirs: EDITOR_DIRS,
    feature: None,
    wit: &[
        "wit/kai-ui",
        "wit/kai-edit",
        "wit/kai-files",
        "wit/kai-code",
        EDITOR_WIT,
    ],
};

/// The label every window's one version has on the boot drive.
pub const WINDOW_LABEL: &str = "1";

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

/// Every version that ships on the boot drive: `notes` 1 and 2 and the apps.
/// The broken `notes` versions the M9a test expects to roll back are
/// [`TEST_ONLY`].
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
    // The windows P4 ships, each also a package of the image's repository.
    Version {
        label: WINDOW_LABEL,
        recipe: NOTES_WINDOW_RECIPE,
    },
    Version {
        label: WINDOW_LABEL,
        recipe: FILES_WINDOW_RECIPE,
    },
    Version {
        label: WINDOW_LABEL,
        recipe: TERMINAL_WINDOW_RECIPE,
    },
    Version {
        label: WINDOW_LABEL,
        recipe: SETTINGS_WINDOW_RECIPE,
    },
    Version {
        label: WINDOW_LABEL,
        recipe: EDITOR_RECIPE,
    },
    Version {
        label: WINDOW_LABEL,
        recipe: MONITOR_WINDOW_RECIPE,
    },
    Version {
        label: WINDOW_LABEL,
        recipe: MODELS_WINDOW_RECIPE,
    },
];

/// Versions only the test images carry: a fixture that exists to be driven by a
/// boot case, never shipped. [`build`] leaves them out, so the release and slot
/// images, the VM image and the package repository never hold one;
/// [`build_for_tests`] adds them for the images `cargo xtask test` boots, and
/// [`check_no_test_only`] fails an image that has one. A version here is not also
/// in [`VERSIONS`].
pub const TEST_ONLY: &[Version] = &[
    // `notes` versions that fail to import, fail their health check, trap and
    // spin: live-update rolls each back (M9a), and `:fault notes` names
    // 3-trap's panic. A release must never offer one to update into.
    bad("3-import", "bug-import"),
    bad("3-health", "bug-health"),
    bad("3-trap", "bug-trap"),
    bad("3-spin", "bug-spin"),
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
    // The `wait-reads` case's window: its buttons queue one `fs_list` or one
    // `fs_create` while the shell waits on the model, and its action says what
    // came of each, in order (the founder's test 2, item 7).
    Version {
        label: "1",
        recipe: Recipe {
            component: "wait-window",
            krate: "comp-wait-fixture",
            dirs: &["apps/wait-fixture"],
            feature: None,
            wit: &[
                "apps/wait-fixture/world",
                "wit/kai-ui",
                "apps/wait-fixture/wit",
            ],
        },
    },
    // The `net-grant` case's headless app: it fetches a fixed list of URLs
    // through `kai:net`, and the case answers the shell's cards for their
    // hosts (P5 task 13).
    Version {
        label: "1",
        recipe: Recipe {
            component: "net-fixture",
            krate: "comp-net-fixture",
            dirs: &["apps/net-fixture"],
            feature: None,
            wit: &["apps/net-fixture/wit", "wit/kai-net"],
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
mod tests;
