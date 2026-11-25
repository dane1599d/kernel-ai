//! Build once, boot on many runners. `cargo xtask build --out DIR` writes
//! what a test run boots (the kernel, the user programs and the component
//! versions) into DIR with a manifest naming each file's SHA-256 and the
//! commit and options it was built with. `cargo xtask test --prebuilt DIR
//! --prebuilt-sha SHA` boots them only when the manifest's own SHA-256 is SHA
//! (the build job's output, never a file the download carries), its commit
//! and options are this run's, and every file hashes as listed; anything else
//! is refused, and the run builds as before. Stale or altered artifacts are
//! never booted.

use crate::esp::Module;
use crate::options::Options;
use crate::sha256_hex;
use std::fs;
use std::path::{Path, PathBuf};

/// The manifest's first line.
const MAGIC: &str = "kai-prebuilt 1";
/// The manifest's name in DIR.
pub const MANIFEST: &str = "manifest.txt";

/// What the artifacts were built from and how: the commit and the options a
/// build takes. `None` for a checkout with changes or no commit, whose builds
/// are never shared.
pub fn build_id(root: &Path, options: &Options) -> Option<String> {
    let commit = crate::esp::commit_id(root);
    if commit.ends_with("-dirty") || commit == "unknown" {
        return None;
    }
    Some(format!(
        "{commit} release={} spikes={}",
        options.release, options.spikes
    ))
}

/// What a test run boots.
pub struct Built {
    pub kernel: PathBuf,
    pub programs: Vec<(&'static str, PathBuf)>,
    pub versions: Vec<Module>,
}

/// What a run boots: `--prebuilt`'s artifacts when they load, built here
/// otherwise; with `build --out`, also written out for other runners.
pub fn obtain(root: &Path, options: &Options) -> Result<Built, String> {
    if let Some((dir, sha)) = &options.prebuilt {
        let loaded = build_id(root, options)
            .ok_or_else(|| "this checkout has changes or no commit".to_string())
            .and_then(|id| load(dir, sha, &id));
        match loaded {
            Ok(built) => {
                println!(
                    "xtask: booting the build job's artifacts from {} (manifest {})",
                    dir.display(),
                    &sha[..12]
                );
                return Ok(built);
            }
            Err(why) => eprintln!(
                "xtask: not booting the artifacts in {}: {why}; building them here",
                dir.display()
            ),
        }
    }
    let kernel = crate::build_kernel(root, options)?;
    let programs = crate::userbuild::build_user(root, options)?;
    let versions = crate::components::build_for_tests(root)?;
    if let Some(dir) = &options.out {
        let id = build_id(root, options).ok_or(
            "build --out: this checkout has changes or no commit; its build is not shared",
        )?;
        let sha = save(dir, &id, &kernel, &programs, &versions)?;
        println!("xtask: wrote {} with manifest sha256 {sha}", dir.display());
    }
    Ok(Built {
        kernel,
        programs,
        versions,
    })
}

/// Writes the artifacts and their manifest into `dir`, emptied first, and
/// returns the manifest's SHA-256.
pub fn save(
    dir: &Path,
    id: &str,
    kernel: &Path,
    programs: &[(&'static str, PathBuf)],
    versions: &[Module],
) -> Result<String, String> {
    // Only an earlier `--out` (or an empty directory) is emptied, never a
    // directory that is something else.
    if let Ok(mut entries) = fs::read_dir(dir) {
        if entries.next().is_some() && !dir.join(MANIFEST).is_file() {
            return Err(format!(
                "{} is not empty and holds no {MANIFEST}; give a new directory",
                dir.display()
            ));
        }
        fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut manifest = format!("{MAGIC}\nbuild {id}\n");
    let mut put = |kind: &str, name: &str, file: &str, bytes: &[u8]| -> Result<(), String> {
        fs::write(dir.join(file), bytes).map_err(|e| format!("{file}: {e}"))?;
        manifest.push_str(&format!("{kind} {} {file} {name}\n", sha256_hex(bytes)));
        Ok(())
    };
    let read = |path: &Path| fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
    put("kernel", "kernel", "kernel.elf", &read(kernel)?)?;
    for (i, (name, elf)) in programs.iter().enumerate() {
        put("program", name, &format!("program-{i}.elf"), &read(elf)?)?;
    }
    for (i, version) in versions.iter().enumerate() {
        put(
            "version",
            &version.path,
            &format!("version-{i}.wasm"),
            &version.bytes,
        )?;
    }
    fs::write(dir.join(MANIFEST), &manifest).map_err(|e| format!("{MANIFEST}: {e}"))?;
    Ok(sha256_hex(manifest.as_bytes()))
}

/// The artifacts in `dir`, when its manifest hashes to `want` (the build
/// job's output), names build `id`, and every file it lists hashes as listed.
pub fn load(dir: &Path, want: &str, id: &str) -> Result<Built, String> {
    let text = fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let got = sha256_hex(text.as_bytes());
    if got != want {
        return Err(format!(
            "the manifest hashes to {got}, not the build job's {want}"
        ));
    }
    let mut lines = text.lines();
    if lines.next() != Some(MAGIC) {
        return Err(format!("{MANIFEST} is not {MAGIC}"));
    }
    match lines.next().and_then(|l| l.strip_prefix("build ")) {
        Some(built) if built == id => {}
        Some(built) => return Err(format!("built as {built}, but this run is {id}")),
        None => return Err(format!("{MANIFEST} names no build")),
    }
    let (mut kernel, mut programs, mut versions) = (None, Vec::new(), Vec::new());
    for line in lines {
        let mut words = line.splitn(4, ' ');
        let (Some(kind), Some(hash), Some(file), Some(name)) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            return Err(format!("a bad manifest line: {line}"));
        };
        if file.contains(['/', '\\']) || file.starts_with('.') {
            return Err(format!("a manifest file outside {}: {file}", dir.display()));
        }
        let path = dir.join(file);
        let bytes = fs::read(&path).map_err(|e| format!("{file}: {e}"))?;
        if sha256_hex(&bytes) != hash {
            return Err(format!(
                "{file} ({name}) does not hash as the manifest says"
            ));
        }
        match kind {
            "kernel" => kernel = Some(path),
            "program" => programs.push((&*Box::leak(name.to_string().into_boxed_str()), path)),
            "version" => versions.push(Module {
                path: name.to_string(),
                bytes,
            }),
            _ => return Err(format!("an unknown manifest kind: {kind}")),
        }
    }
    let kernel = kernel.ok_or("the manifest lists no kernel")?;
    Ok(Built {
        kernel,
        programs,
        versions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (PathBuf, PathBuf, Vec<(&'static str, PathBuf)>, Vec<Module>) {
        let base = std::env::temp_dir().join(format!("kai-prebuilt-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let kernel = base.join("kernel");
        fs::write(&kernel, b"kernel elf").unwrap();
        let shell = base.join("shell");
        fs::write(&shell, b"shell elf").unwrap();
        let versions = vec![Module {
            path: "comp/notes/2.wasm".into(),
            bytes: b"\0asm notes".to_vec(),
        }];
        (base.join("out"), kernel, vec![("shell", shell)], versions)
    }

    #[test]
    fn what_was_saved_loads_back_the_same() {
        let (out, kernel, programs, versions) = fixture("round");
        let sha = save(&out, "abc123", &kernel, &programs, &versions).unwrap();
        let built = load(&out, &sha, "abc123").unwrap();
        assert_eq!(fs::read(&built.kernel).unwrap(), b"kernel elf");
        assert_eq!(built.programs.len(), 1);
        assert_eq!(built.programs[0].0, "shell");
        assert_eq!(fs::read(&built.programs[0].1).unwrap(), b"shell elf");
        assert_eq!(built.versions.len(), 1);
        assert_eq!(built.versions[0].path, "comp/notes/2.wasm");
        assert_eq!(built.versions[0].bytes, b"\0asm notes");
    }

    #[test]
    fn another_manifest_hash_is_refused() {
        let (out, kernel, programs, versions) = fixture("sha");
        save(&out, "abc123", &kernel, &programs, &versions).unwrap();
        let err = load(&out, &"0".repeat(64), "abc123").err().unwrap();
        assert!(err.contains("not the build job's"), "{err}");
    }

    #[test]
    fn artifacts_of_another_commit_are_refused() {
        let (out, kernel, programs, versions) = fixture("commit");
        let sha = save(&out, "abc123", &kernel, &programs, &versions).unwrap();
        let err = load(&out, &sha, "def456").err().unwrap();
        assert!(err.contains("built as abc123"), "{err}");
    }

    #[test]
    fn a_changed_artifact_is_refused() {
        let (out, kernel, programs, versions) = fixture("bytes");
        let sha = save(&out, "abc123", &kernel, &programs, &versions).unwrap();
        fs::write(out.join("program-0.elf"), b"another shell").unwrap();
        let err = load(&out, &sha, "abc123").err().unwrap();
        assert!(err.contains("does not hash"), "{err}");
    }

    #[test]
    fn a_directory_that_is_something_else_is_never_emptied() {
        let (out, kernel, programs, versions) = fixture("other");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("notes.txt"), b"mine").unwrap();
        assert!(save(&out, "abc123", &kernel, &programs, &versions).is_err());
        assert_eq!(fs::read(out.join("notes.txt")).unwrap(), b"mine");
    }

    #[test]
    fn a_missing_download_is_refused() {
        let (out, ..) = fixture("missing");
        assert!(load(&out, &"0".repeat(64), "abc123").is_err());
    }
}
