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
];

/// Builds and packs every version.
pub fn build(root: &Path) -> Result<Vec<Module>, String> {
    check_target()?;
    VERSIONS
        .iter()
        .map(|version| build_one(root, version))
        .collect()
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
        path: format!("comp/{}/{}.wasm", version.recipe.component, version.label),
        bytes,
    })
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
    let world = format!("app:{0}/{0}@1.0.0", recipe.component);
    let dirs: Vec<PathBuf> = KAI_WIT
        .iter()
        .chain(recipe.wit)
        .map(|dir| root.join(dir))
        .collect();
    let actions = crate::kai_actions::write(&dirs, &world)?;
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
