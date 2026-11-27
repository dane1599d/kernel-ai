//! `cargo xtask notices [--check]` (ADR 0009 D6): writes `NOTICES.txt`, the
//! notices every part of the image owes, from
//! 1. cargo-about 0.9.2 over the shipped graphs: the kernel (with the spikes)
//!    and the ring-3 programs for `x86_64-unknown-none`, and the components for
//!    `wasm32-unknown-unknown`, normal dependencies only, no proc-macros;
//! 2. the hand list `xtask/notices.json`: parts no Cargo.lock entry covers, with
//!    their texts from `LICENSES/` or, for Limine, verbatim from its pinned tarball.
//!
//! The file names no hash of `Cargo.lock`, so it changes only when a shipped crate,
//! its version or a licence does; a PR that adds a crate of our own leaves it alone.
//! `--check-sbom FILE` checks the release's CycloneDX SBOM against it: every crate
//! of its Parts is a component, and every registry component's licence is one
//! `deny.toml` allows (see [`sbom_problems`]); it reads the committed file and
//! runs no cargo-about, which the release's build runners do not have.
//! `--check` regenerates it and fails when the committed file differs; CI runs
//! it, so only CI and whoever changes dependencies need cargo-about. `make_esp`
//! copies the file to the boot drive with [`image_line`] on top.

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
    dispatch(&crate::workspace_root(), args, render)
}

/// [`main`] with the notices made by `render`: only writing and `--check` make
/// them (cargo-about); `--check-sbom` never does.
fn dispatch(
    root: &Path,
    args: &[String],
    render: impl FnOnce(&Path) -> Result<String, String>,
) -> Result<(), String> {
    if let [flag, sbom] = args
        && flag == "--check-sbom"
    {
        return check_sbom(root, Path::new(sbom));
    }
    let text = render(root)?;
    let path = root.join(FILE);
    match args {
        [] => {
            fs::write(&path, &text).map_err(|e| format!("cannot write {FILE}: {e}"))?;
            println!("xtask: wrote {FILE} ({} KiB)", text.len() / 1024);
            Ok(())
        }
        [flag] if flag == "--check" => {
            let committed = fs::read_to_string(&path).unwrap_or_default();
            if let Some(lines) = stale(&committed, &text) {
                // The first lines that differ, so a stale file can be fixed from
                // CI's log alone.
                for line in lines {
                    println!("xtask: {line}");
                }
                return Err(format!(
                    "{FILE} is stale: run `cargo xtask notices` and commit it"
                ));
            }
            println!("xtask: {FILE} is up to date");
            Ok(())
        }
        _ => Err("usage: cargo xtask notices [--check | --check-sbom FILE]".into()),
    }
}

/// `None` when the committed notices match the generated ones, line endings
/// aside; else the first lines that differ and both line counts, for the log.
fn stale(committed: &str, generated: &str) -> Option<Vec<String>> {
    let committed = committed.replace("\r\n", "\n");
    if committed == generated {
        return None;
    }
    let pairs = committed.lines().zip(generated.lines()).enumerate();
    let mut lines: Vec<String> = pairs
        .filter(|(_, (old, new))| old != new)
        .take(8)
        .flat_map(|(at, (old, new))| {
            [
                format!("{FILE}:{}: committed: {old}", at + 1),
                format!("{FILE}:{}: generated: {new}", at + 1),
            ]
        })
        .collect();
    let (old, new) = (committed.lines().count(), generated.lines().count());
    lines.push(format!("{FILE}: {old} lines committed, {new} generated"));
    Some(lines)
}

/// `--check-sbom`: reads the SBOM at `path` and fails naming every problem.
fn check_sbom(root: &Path, path: &Path) -> Result<(), String> {
    let sbom = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let notices =
        fs::read_to_string(root.join(FILE)).map_err(|e| format!("cannot read {FILE}: {e}"))?;
    let problems = sbom_problems(&notices, &sbom, &crate::licences::rules(root)?)?;
    for problem in &problems {
        println!("xtask: {problem}");
    }
    match problems.is_empty() {
        true => {
            println!("xtask: the SBOM lists every crate of {FILE}, each under an allowed licence");
            Ok(())
        }
        false => Err(format!(
            "the SBOM and {FILE} disagree ({} problems)",
            problems.len()
        )),
    }
}

/// The crates of `notices`' Parts, as (name, version), and the lines that start
/// like a crate (`- name 1.2`) but did not parse as one (`- name 1.2: licence.`).
/// The other Parts (core, Limine, the fonts) have no Cargo.lock entry.
fn parts_crates(notices: &str) -> (BTreeSet<(String, String)>, Vec<String>) {
    let (mut crates, mut unparsed) = (BTreeSet::new(), Vec::new());
    let parts = notices
        .lines()
        .skip_while(|line| *line != "Parts")
        .take_while(|line| *line != "Licence texts");
    for line in parts.filter_map(|line| line.strip_prefix("- ")) {
        let mut words = line.splitn(3, ' ');
        let (Some(name), Some(rest)) = (words.next(), words.next()) else {
            continue;
        };
        let name_ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
        if !name_ok || !rest.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        match rest.strip_suffix(':') {
            Some(version) if !version.contains([' ', ':']) && words.next().is_some() => {
                crates.insert((name.to_owned(), version.to_owned()));
            }
            _ => unparsed.push(format!("- {line}")),
        }
    }
    (crates, unparsed)
}

/// What is wrong between `notices` and the CycloneDX `sbom`: a crate the
/// notices list that is no component, a Parts line that looks like a crate and
/// does not parse, and a registry component with no licence or one that
/// `deny.toml`'s rules do not allow. Our own crates (a `file://` purl) are
/// listed in the SBOM and not judged: their licence is the workspace's.
pub fn sbom_problems(
    notices: &str,
    sbom: &str,
    rules: &crate::licences::Rules,
) -> Result<Vec<String>, String> {
    let sbom: serde_json::Value =
        serde_json::from_str(sbom).map_err(|e| format!("the SBOM is not JSON: {e}"))?;
    let components = sbom["components"]
        .as_array()
        .ok_or("the SBOM has no components")?;
    let field = |c: &serde_json::Value, k: &str| c[k].as_str().unwrap_or_default().to_owned();
    let listed: BTreeSet<(String, String)> = components
        .iter()
        .map(|c| (field(c, "name"), field(c, "version")))
        .collect();
    let (noticed, unparsed) = parts_crates(notices);
    if noticed.is_empty() {
        return Err(format!("{FILE} lists no crates: its Parts did not parse"));
    }
    let mut problems: Vec<String> = unparsed
        .into_iter()
        .map(|line| format!("{FILE} has a crate-like line that does not parse: {line}"))
        .collect();
    for (name, version) in noticed.difference(&listed) {
        problems.push(format!("{FILE} lists {name} {version}; the SBOM does not"));
    }
    for component in components {
        let (name, version) = (field(component, "name"), field(component, "version"));
        if field(component, "purl").contains("file://") {
            continue;
        }
        let expression = rules.clarified(&name).map(str::to_owned).or_else(|| {
            component["licenses"][0]["expression"]
                .as_str()
                .or_else(|| component["licenses"][0]["license"]["id"].as_str())
                .map(str::to_owned)
        });
        match expression {
            None => problems.push(format!("{name} {version} has no licence in the SBOM")),
            Some(expression) => {
                if let Err(why) = crate::licences::choose(&expression, rules, Some(&name)) {
                    problems.push(format!(
                        "{name} {version}: `{expression}` is not allowed by deny.toml: {why}"
                    ));
                }
            }
        }
    }
    Ok(problems)
}

/// The OS's version: `version` under `[workspace.package]` in `root`'s
/// `Cargo.toml`, the one every crate inherits (ADR 0009 D7).
pub fn workspace_version(root: &Path) -> Result<String, String> {
    let path = root.join("Cargo.toml");
    let toml =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    toml.lines()
        .skip_while(|line| line.trim() != "[workspace.package]")
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "version").then(|| value.trim().trim_matches('"').to_owned())
        })
        .filter(|version| !version.is_empty())
        .ok_or_else(|| format!("{} has no [workspace.package] version", path.display()))
}

/// The line `make_esp` puts above the notices on the boot drive. The version is
/// the workspace's one (`[workspace.package]`, ADR 0009 D7).
pub fn image_line(root: &Path) -> String {
    let version = workspace_version(root).unwrap_or_else(|_| "unknown".into());
    let commit = crate::esp::commit_id(root);
    format!(
        "Kernel AI {version} (commit {commit}). Licence MPL-2.0. \
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
    fn the_image_line_names_the_workspace_version() {
        let line = image_line(&crate::workspace_root());
        assert!(line.starts_with("Kernel AI 0.1.0-dev (commit "), "{line}");
    }

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
    fn check_catches_a_new_dependency_and_ignores_line_endings() {
        let committed = "Parts\n\n- smoltcp 0.14.0: 0BSD.\n- spin 0.12.3: MIT.\n";
        assert_eq!(stale(committed, committed), None);
        assert_eq!(stale(&committed.replace('\n', "\r\n"), committed), None);
        // A fake dependency entry the committed file lacks.
        let generated = committed.replace("- spin", "- fakedep 1.0.0: MIT.\n- spin");
        let lines = stale(committed, &generated).expect("a stale file");
        assert_eq!(lines[0], "NOTICES.txt:4: committed: - spin 0.12.3: MIT.");
        assert_eq!(lines[1], "NOTICES.txt:4: generated: - fakedep 1.0.0: MIT.");
        assert_eq!(
            lines.last().unwrap(),
            "NOTICES.txt: 4 lines committed, 5 generated"
        );
        // A changed licence, same line count.
        let relicensed = committed.replace("spin 0.12.3: MIT", "spin 0.12.3: Apache-2.0");
        assert!(stale(committed, &relicensed).is_some());
    }

    fn deny() -> crate::licences::Rules {
        crate::licences::rules(&crate::workspace_root()).unwrap()
    }

    fn sbom(components: &[(&str, &str, &str)]) -> String {
        let list: Vec<_> = components
            .iter()
            .map(|(name, version, licence)| {
                serde_json::json!({
                    "name": name, "version": version,
                    "purl": format!("pkg:cargo/{name}@{version}"),
                    "licenses": [{ "expression": licence }],
                })
            })
            .collect();
        serde_json::json!({ "components": list }).to_string()
    }

    const NOTICES: &str = concat!(
        "Parts\n\n",
        "- core and alloc (rust-src): MIT OR Apache-2.0. In the kernel.\n",
        "- smoltcp 0.14.0: 0BSD.\n",
        "- spin 0.12.3: MIT.\n",
        "\nLicence texts\n",
        "- zzz 9.9.9: MIT.\n",
    );

    #[test]
    fn parts_take_crate_lines_only_and_stop_at_the_licence_texts() {
        let (crates, unparsed) = parts_crates(NOTICES);
        let names: Vec<_> = crates.iter().map(|(n, v)| format!("{n} {v}")).collect();
        assert_eq!(names, ["smoltcp 0.14.0", "spin 0.12.3"]);
        assert!(unparsed.is_empty(), "{unparsed:?}");
    }

    #[test]
    fn a_crate_like_line_that_does_not_parse_is_named() {
        let notices = "Parts\n- smoltcp 0.14.0 (x): 0BSD.\n- Limine (12.9): BSD-2-Clause.\n";
        let (crates, unparsed) = parts_crates(notices);
        assert!(crates.is_empty());
        assert_eq!(unparsed, ["- smoltcp 0.14.0 (x): 0BSD."]);
    }

    #[test]
    fn an_sbom_with_every_crate_under_an_allowed_licence_has_no_problem() {
        let s = sbom(&[
            ("smoltcp", "0.14.0", "0BSD"),
            ("spin", "0.12.3", "MIT OR Apache-2.0"),
        ]);
        assert_eq!(
            sbom_problems(NOTICES, &s, &deny()).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_crate_missing_from_the_sbom_is_named() {
        let s = sbom(&[("smoltcp", "0.14.0", "0BSD"), ("spin", "0.12.2", "MIT")]);
        let problems = sbom_problems(NOTICES, &s, &deny()).unwrap();
        assert_eq!(
            problems,
            ["NOTICES.txt lists spin 0.12.3; the SBOM does not"]
        );
    }

    #[test]
    fn a_licence_deny_toml_does_not_allow_is_named() {
        let s = sbom(&[
            ("smoltcp", "0.14.0", "0BSD"),
            ("spin", "0.12.3", "MIT"),
            ("copyleft", "1.0.0", "GPL-3.0-only"),
            ("bare", "1.0.0", ""),
        ]);
        let problems = sbom_problems(NOTICES, &s, &deny()).unwrap();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("copyleft 1.0.0: `GPL-3.0-only` is not allowed"));
        assert!(problems[1].starts_with("bare 1.0.0: ``"), "{problems:?}");
    }

    #[test]
    fn our_own_crates_and_a_crate_with_no_licence_are_told_apart() {
        let own = serde_json::json!({ "components": [
            {
                "name": "kabi", "version": "0.1.0-dev",
                "purl": "pkg:cargo/kabi@0.1.0-dev?download_url=file://../kabi",
            },
            { "name": "smoltcp", "version": "0.14.0", "purl": "pkg:cargo/smoltcp@0.14.0" },
            {
                "name": "spin", "version": "0.12.3", "purl": "pkg:cargo/spin@0.12.3",
                "licenses": [{ "license": { "id": "MIT" } }],
            },
        ]})
        .to_string();
        let problems = sbom_problems(NOTICES, &own, &deny()).unwrap();
        assert_eq!(problems, ["smoltcp 0.14.0 has no licence in the SBOM"]);
    }

    /// #1557's review: the release's build runners have no cargo-about, so
    /// `--check-sbom` through `main`'s dispatch must never render the notices.
    /// A temp directory that goes when the test ends, a failed assert included.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("kai-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn check_sbom_runs_no_cargo_about() {
        let root = crate::workspace_root();
        let notices = fs::read_to_string(root.join(FILE)).unwrap();
        let (crates, _) = parts_crates(&notices);
        let components: Vec<(&str, &str, &str)> = crates
            .iter()
            .map(|(name, version)| (name.as_str(), version.as_str(), "MIT"))
            .collect();
        let scratch = Scratch::new("check-sbom-no-cargo-about");
        let dir = &scratch.0;
        let (good, short) = (dir.join("good.cdx.json"), dir.join("short.cdx.json"));
        fs::write(&good, sbom(&components)).unwrap();
        fs::write(&short, sbom(&components[1..])).unwrap();
        let args = |path: &Path| ["--check-sbom".to_owned(), path.display().to_string()];
        let never = |_: &Path| -> Result<String, String> { panic!("cargo-about ran") };
        assert_eq!(dispatch(&root, &args(&good), never), Ok(()));
        let err = dispatch(&root, &args(&short), never).unwrap_err();
        assert!(err.contains("(1 problems)"), "{err}");
    }

    #[test]
    fn the_committed_notices_parse_with_no_crate_like_line_left_over() {
        let notices = fs::read_to_string(crate::workspace_root().join(FILE)).unwrap();
        let (crates, unparsed) = parts_crates(&notices);
        assert!(crates.len() > 100, "{}", crates.len());
        assert!(unparsed.is_empty(), "{unparsed:?}");
    }

    #[test]
    fn the_notices_name_no_lock_hash() {
        let notices = fs::read_to_string(crate::workspace_root().join(FILE)).unwrap();
        assert!(!notices.contains("Cargo.lock"));
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
