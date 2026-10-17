//! Live component versions: each built for wasm32 with the `component` profile,
//! packed into an artifact (the module, its manifest and its source) and shipped
//! as boot module `comp/<component>/<label>.wasm`.

use crate::esp::Module;
use crate::sha256_hex;
use kabi::live::{MANIFEST_SECTION, SOURCE_SECTION};
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
}

/// One version on the boot drive.
pub struct Version {
    pub label: &'static str,
    pub recipe: Recipe,
}

const NOTES: &str = "components/notes";
const BAD: &str = "components/fixtures/notes-bad";

/// `notes` from `components/notes`: version 2, and every upload the edit loop
/// builds.
pub const NOTES_RECIPE: Recipe = Recipe {
    component: "notes",
    krate: "comp-notes",
    dirs: &[NOTES],
    feature: None,
};

const fn bad(label: &'static str, feature: &'static str) -> Version {
    Version {
        label,
        recipe: Recipe {
            component: "notes",
            krate: "comp-notes-bad",
            dirs: &[BAD, NOTES],
            feature: Some(feature),
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
    let bytes = pack(&version.recipe, version.label, module, &source);
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

/// The module with its manifest and source sections appended.
pub fn pack(recipe: &Recipe, label: &str, mut module: Vec<u8>, source: &[u8]) -> Vec<u8> {
    let manifest = Manifest {
        component: recipe.component.into(),
        version: label.into(),
        interface: format!("{}@1", recipe.component),
        features: recipe.feature.unwrap_or("-").into(),
        build: sha256_hex(&module),
        source: sha256_hex(source),
    };
    artifact::append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
    artifact::append_custom_section(&mut module, SOURCE_SECTION, source);
    module
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
            assert!(VERSIONS[..index].iter().all(|v| v.label != version.label));
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

    #[test]
    fn a_packed_module_keeps_its_bytes_and_says_what_it_is() {
        let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
        let source = artifact::bundle(&[("components/notes/src/lib.rs", b"//")]);
        let bytes = pack(
            &VERSIONS[2].recipe,
            VERSIONS[2].label,
            module.clone(),
            &source,
        );
        let parsed = artifact::parse(&bytes).unwrap();
        assert!(bytes.starts_with(&module));
        assert_eq!(parsed.manifest.version, "3-import");
        assert_eq!(parsed.manifest.features, "bug-import");
        assert_eq!(parsed.manifest.build, sha256_hex(&module));
        assert_eq!(parsed.source, &source[..]);
    }
}
