//! `cargo xtask notices [--check]` (ADR 0009 D6): writes `NOTICES.txt`, the
//! notices every part of the image owes, from
//! 1. cargo-about 0.9.2 over the shipped graphs: the kernel (with the spikes)
//!    and the ring-3 programs for `x86_64-unknown-none`, and the components for
//!    `wasm32-unknown-unknown`, normal dependencies only, no proc-macros;
//! 2. the hand list `xtask/notices.json`: parts no Cargo.lock entry covers, with
//!    their texts from `LICENSES/` or, for Limine, verbatim from its pinned tarball;
//! 3. a last line with the SHA-256 of `Cargo.lock`.
//!
//! `--check` fails when the committed file is stale; CI runs it, so only CI and
//! whoever changes dependencies need cargo-about. `make_esp` copies the file to
//! the boot drive with [`image_line`] on top.

use crate::licences;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

pub const FILE: &str = "NOTICES.txt";
const REPOSITORY: &str = "https://github.com/dane1599d/kernel-ai";
const RULE: &str =
    "--------------------------------------------------------------------------------";

/// The shipped graphs: package, features, target.
const GRAPHS: [(&str, &str, &str); 3] = [
    ("kernel", "spikes", crate::KERNEL_TARGET),
    ("user", "", crate::KERNEL_TARGET),
    ("comp-notes", "", "wasm32-unknown-unknown"),
];

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let text = render(&root)?;
    let path = root.join(FILE);
    match args {
        [] => {
            fs::write(&path, &text).map_err(|e| format!("cannot write {FILE}: {e}"))?;
            println!("xtask: wrote {FILE} ({} KiB)", text.len() / 1024);
            Ok(())
        }
        [flag] if flag == "--check" => {
            let committed = fs::read_to_string(&path)
                .unwrap_or_default()
                .replace("\r\n", "\n");
            if committed != text {
                // The first lines that differ, so a stale file can be fixed from
                // CI's log alone.
                let pairs = committed.lines().zip(text.lines()).enumerate();
                for (at, (old, new)) in pairs.filter(|(_, (o, n))| o != n).take(8) {
                    println!("xtask: {FILE}:{}: committed: {old}", at + 1);
                    println!("xtask: {FILE}:{}: generated: {new}", at + 1);
                }
                let (old, new) = (committed.lines().count(), text.lines().count());
                println!("xtask: {FILE}: {old} lines committed, {new} generated");
                return Err(format!(
                    "{FILE} is stale: run `cargo xtask notices` and commit it"
                ));
            }
            println!("xtask: {FILE} is up to date");
            Ok(())
        }
        _ => Err("usage: cargo xtask notices [--check]".into()),
    }
}

/// The line `make_esp` puts above the notices on the boot drive.
pub fn image_line(root: &Path) -> String {
    let version = fs::read_to_string(root.join("kernel/Cargo.toml"))
        .ok()
        .and_then(|toml| {
            toml.lines().find_map(|l| {
                l.strip_prefix("version = ")
                    .map(|v| v.trim_matches('"').to_owned())
            })
        })
        .unwrap_or_else(|| "unknown".into());
    let commit = crate::esp::commit_id(root);
    format!(
        "Kernel AI {version} (commit {commit}). Licence: not yet chosen (all rights reserved). \
         Source: {REPOSITORY}/tree/{}\n\n",
        commit.trim_end_matches("-dirty")
    )
}

/// One part of the hand list.
struct Part {
    name: String,
    version: String,
    licence: String,
    /// The crate whose approved exception covers this part, if any.
    krate: Option<String>,
    copyright: Vec<String>,
    within: String,
    verbatim: Vec<String>,
}

fn render(root: &Path) -> Result<String, String> {
    let rules = licences::rules(root)?;
    let parts = hand_list(root)?;
    let shipped = shipped(root)?;
    let mut texts: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut crates: BTreeMap<(String, String), String> = BTreeMap::new();
    for (package, features, target) in GRAPHS {
        for license in about(root, package, features, target)? {
            let id = license["id"].as_str().unwrap_or_default().to_owned();
            let text = license["text"]
                .as_str()
                .unwrap_or_default()
                .replace("\r\n", "\n");
            for used in license["used_by"].as_array().into_iter().flatten() {
                let krate = &used["crate"];
                let name = krate["name"].as_str().unwrap_or_default().to_owned();
                let version = krate["version"].as_str().unwrap_or_default().to_owned();
                if !shipped.contains(&(name.clone(), version.clone())) {
                    continue;
                }
                let expression = krate["license"]
                    .as_str()
                    .unwrap_or("NOASSERTION")
                    .to_owned();
                texts
                    .entry((id.clone(), text.trim().to_owned()))
                    .or_default()
                    .insert(format!("{name} {version}"));
                crates.insert((name, version), expression);
            }
        }
    }
    if let Some((name, version)) = shipped.iter().find(|k| !crates.contains_key(*k)) {
        return Err(format!(
            "cargo-about gave no licence text for {name} {version}"
        ));
    }
    let mut out = String::from(
        "Third-party notices for Kernel AI\n\n\
         Parts of Kernel AI come from other projects under their own licences. Each part\n\
         is listed below, then each licence text once, with the parts it covers.\n\n",
    );
    out.push_str("Parts\n\n");
    for part in &parts {
        out.push_str(&format!(
            "- {} ({}): {}. In {}.\n",
            part.name, part.version, part.licence, part.within
        ));
    }
    for ((name, version), expression) in &crates {
        out.push_str(&format!("- {name} {version}: {expression}.\n"));
    }
    out.push_str("\nLicence texts\n");
    for part in &parts {
        out.push_str(&format!(
            "\n{RULE}\n{} ({})\n{RULE}\n\n",
            part.name, part.licence
        ));
        for line in &part.copyright {
            out.push_str(&format!("{line}\n"));
        }
        if !part.copyright.is_empty() {
            out.push('\n');
        }
        if part.verbatim.is_empty() {
            let mut ids = licences::choose(&part.licence, &rules, part.krate.as_deref())
                .map_err(|e| format!("notices.json: {}: {e}", part.name))?;
            let mut seen = BTreeSet::new();
            ids.retain(|id| seen.insert(id.clone()));
            for id in ids {
                let path = root.join("LICENSES").join(format!("{id}.txt"));
                let text =
                    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                out.push_str(&format!(
                    "{id}:\n\n{}\n\n",
                    text.replace("\r\n", "\n").trim()
                ));
            }
        } else {
            let dir = limine_dir(root, &part.version)?;
            for file in &part.verbatim {
                let text = fs::read_to_string(dir.join(file))
                    .map_err(|e| format!("Limine {file}: {e}"))?;
                out.push_str(&format!(
                    "{file}:\n\n{}\n\n",
                    text.replace("\r\n", "\n").trim()
                ));
            }
        }
    }
    for ((id, text), users) in &texts {
        let users: Vec<&str> = users.iter().map(String::as_str).collect();
        out.push_str(&format!(
            "\n{RULE}\n{id}: {}\n{RULE}\n\n{text}\n",
            users.join(", ")
        ));
    }
    let lock = fs::read(root.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    let lock = String::from_utf8_lossy(&lock).replace("\r\n", "\n");
    out.push_str(&format!(
        "\n{RULE}\nCargo.lock SHA-256: {}\n",
        crate::sha256_hex(lock.as_bytes())
    ));
    Ok(out)
}

fn hand_list(root: &Path) -> Result<Vec<Part>, String> {
    let path = root.join("xtask/notices.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("notices.json: {e}"))?;
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect()
    };
    let mut parts = Vec::new();
    for part in json["parts"].as_array().ok_or("notices.json: no parts")? {
        let field = |k: &str| {
            part[k]
                .as_str()
                .map(str::to_owned)
                .ok_or(format!("notices.json: a part without `{k}`"))
        };
        parts.push(Part {
            name: field("name")?,
            version: field("version")?,
            licence: field("licence")?,
            krate: part["crate"].as_str().map(str::to_owned),
            copyright: strings(&part["copyright"]),
            within: field("in")?,
            verbatim: strings(&part["verbatim"]),
        });
    }
    Ok(parts)
}

/// The hand list's parts for `licences check`: name, approving crate, licence.
pub fn hand_expressions(root: &Path) -> Result<Vec<(String, Option<String>, String)>, String> {
    let parts = hand_list(root)?.into_iter();
    Ok(parts.map(|p| (p.name, p.krate, p.licence)).collect())
}

/// Limine's extracted tarball, fetched and hash-checked if need be.
fn limine_dir(root: &Path, version: &str) -> Result<std::path::PathBuf, String> {
    if version != crate::LIMINE_VERSION {
        return Err(format!(
            "notices.json names Limine {version}, but xtask pins {}",
            crate::LIMINE_VERSION
        ));
    }
    let efi = crate::ensure_limine(&root.join("target"))?;
    Ok(efi.parent().ok_or("Limine has no directory")?.to_path_buf())
}

/// Every registry crate in the shipped graphs: normal dependencies only, no
/// proc-macros, no crates of our own.
fn shipped(root: &Path) -> Result<BTreeSet<(String, String)>, String> {
    let mut all = BTreeSet::new();
    for (package, features, target) in GRAPHS {
        let mut args = vec![
            "tree",
            "--locked",
            "-e",
            "normal,no-proc-macro",
            "--prefix",
            "none",
        ];
        args.extend(["--format", "{p}", "--target", target, "-p", package]);
        let features = format!("{package}/{features}");
        if !features.ends_with('/') {
            args.extend(["--features", &features]);
        }
        all.extend(cargo(root, &args)?.lines().filter_map(registry_crate));
    }
    Ok(all)
}

/// The name and version on a `cargo tree --prefix none` line, unless the
/// crate is ours or from git: those print their source, `name v0.0.0 (path)`.
fn registry_crate(line: &str) -> Option<(String, String)> {
    let mut words = line.split_whitespace();
    let (name, version) = (words.next()?, words.next()?);
    if words
        .next()
        .is_some_and(|w| w.starts_with('(') && w != "(*)")
    {
        return None;
    }
    Some((name.to_owned(), version.trim_start_matches('v').to_owned()))
}

/// cargo-about's licences for one graph, offline so every runner agrees.
fn about(
    root: &Path,
    package: &str,
    features: &str,
    target: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let manifest = member_manifest(root, package)?;
    let mut args = vec![
        "about",
        "generate",
        "--format",
        "json",
        "--frozen",
        "-c",
        "about.toml",
    ];
    args.extend(["-m", &manifest, "--target", target]);
    if !features.is_empty() {
        args.extend(["--features", features]);
    }
    let json: serde_json::Value = serde_json::from_str(&cargo(root, &args)?)
        .map_err(|e| format!("cargo about ({package}): {e}"))?;
    Ok(json["licenses"].as_array().cloned().unwrap_or_default())
}

/// The manifest of the workspace member `package`.
fn member_manifest(root: &Path, package: &str) -> Result<String, String> {
    let metadata = cargo(
        root,
        &["metadata", "--format-version", "1", "--locked", "--no-deps"],
    )?;
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata).map_err(|e| format!("cargo metadata: {e}"))?;
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == package)
        .and_then(|p| p["manifest_path"].as_str().map(str::to_owned))
        .ok_or(format!("no workspace member {package}"))
}

fn cargo(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo {}: {e}", args[0]))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("cargo {}: {err}", args.join(" ")));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("cargo {}: {e}", args[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_registry_crates_count_as_shipped() {
        let pair = |n: &str, v: &str| Some((n.to_owned(), v.to_owned()));
        assert_eq!(registry_crate("smoltcp v0.14.0"), pair("smoltcp", "0.14.0"));
        assert_eq!(registry_crate("spin v0.12.3 (*)"), pair("spin", "0.12.3"));
        assert_eq!(registry_crate(r"kabi v0.0.0 (C:\repo\kabi)"), None);
        assert_eq!(registry_crate("kernel v0.10.0 (/repo/kernel)"), None);
        assert_eq!(registry_crate(""), None);
    }

    #[test]
    fn the_hand_list_parses_and_names_the_pinned_limine() {
        let parts = hand_list(&crate::workspace_root()).unwrap();
        let limine = parts.iter().find(|p| p.name == "Limine").unwrap();
        assert_eq!(limine.version, crate::LIMINE_VERSION);
        assert!(
            parts
                .iter()
                .all(|p| !p.licence.is_empty() && !p.within.is_empty())
        );
    }
}
