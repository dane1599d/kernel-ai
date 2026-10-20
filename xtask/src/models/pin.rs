//! `cargo xtask model pin REPO FILE`: fills a catalog entry from Hugging Face.
//! The commit, size and SHA-256 come from the Hub's API, and the file's header
//! comes from a range request of its first megabytes, so nothing large is
//! downloaded. A file the engine cannot run is refused: a tensor type other than
//! F32 or Q8_0, a shape or tensor qwen3 does not have, or a tokenizer other than
//! byte-level BPE with the qwen2 pretokenizer.

use super::ModelArgs;
use super::catalog::{CATALOG_PATH, Entry, is_hex, plain_name};
use serde_json::Value;
use std::fs;
use std::process::Command;

/// The first range request; the Qwen3 header is about 6 MB.
const FIRST_BYTES: u64 = 8 << 20;
const MOST_BYTES: u64 = 64 << 20;

pub fn run(args: &ModelArgs) -> Result<(), String> {
    let [repo, file] = args.words.as_slice() else {
        return Err("model pin expects REPO FILE, for example: \
                    cargo xtask model pin Qwen/Qwen3-0.6B-GGUF Qwen3-0.6B-Q8_0.gguf"
            .into());
    };
    let revision = args.revision.as_deref().unwrap_or("main");
    // They go into URLs, so only plain names pass.
    let repo_ok = repo.split('/').count() == 2 && repo.split('/').all(plain_name);
    if !repo_ok || !plain_name(file) || !file.ends_with(".gguf") || !plain_name(revision) {
        return Err(format!(
            "model pin expects an owner/name repo, a .gguf file name and a plain revision, \
             got {repo:?} {file:?} {revision:?}"
        ));
    }
    let info = api(&format!(
        "https://huggingface.co/api/models/{repo}/revision/{revision}"
    ))?;
    let commit = info["sha"]
        .as_str()
        .filter(|s| is_hex(s, 40))
        .ok_or("the Hub gave no commit for that revision")?
        .to_string();
    let tree = api(&format!(
        "https://huggingface.co/api/models/{repo}/tree/{commit}"
    ))?;
    let entry = entry_from(repo, file, &commit, &info, &tree, args)?;
    println!(
        "xtask: {repo} at {commit}: {file}, {} bytes, SHA-256 {}",
        entry.size, entry.sha256
    );
    engine_runs(&entry.url(), entry.size)?;
    let json = serde_json::to_string_pretty(&entry.to_json()).map_err(|e| e.to_string())?;
    println!("{json}");
    if args.write {
        write(&entry)?;
        println!("xtask: wrote {} into {CATALOG_PATH}", entry.id);
    } else {
        println!("xtask: add it to {CATALOG_PATH}, or run again with --write");
    }
    Ok(())
}

/// The catalog entry for `file` from the Hub's revision and tree answers. The id
/// defaults to the file name in lowercase with `_` turned into `-`
/// (qwen3-0.6b-q8-0), and the name puts a space before the quantisation.
fn entry_from(
    repo: &str,
    file: &str,
    commit: &str,
    info: &Value,
    tree: &Value,
    args: &ModelArgs,
) -> Result<Entry, String> {
    let item = tree
        .as_array()
        .and_then(|items| items.iter().find(|i| i["path"] == file))
        .ok_or(format!("{repo} has no {file} at {commit}"))?;
    let lfs = &item["lfs"];
    let sha256 = lfs["oid"]
        .as_str()
        .filter(|s| is_hex(s, 64))
        .ok_or("the file is not in LFS")?;
    let size = lfs["size"].as_u64().ok_or("the Hub gave no size")?;
    let license = spdx(info["cardData"]["license"].as_str().unwrap_or(""));
    if license != "Apache-2.0" && license != "MIT" {
        return Err(format!(
            "{file} is under {license:?}; the project takes Apache-2.0 or MIT models only"
        ));
    }
    let min_ram_gb = args
        .min_ram
        .ok_or("pass --min-ram GB, the least RAM to recommend this model for")?;
    let stem = file.trim_end_matches(".gguf");
    let id = match &args.id {
        Some(id) => id.clone(),
        None => stem.to_lowercase().replace('_', "-"),
    };
    let name = match stem.rsplit_once('-') {
        Some((model, quant)) => format!("{model} {quant}"),
        None => stem.to_string(),
    };
    let entry = Entry {
        id,
        name,
        repo: repo.to_string(),
        revision: commit.to_string(),
        file: file.to_string(),
        size,
        sha256: sha256.to_string(),
        license,
        min_ram_gb,
        note: String::new(),
    };
    entry.check()?;
    Ok(entry)
}

/// A licence in SPDX spelling. Apache-2.0 and MIT are the ones the project takes.
fn spdx(license: &str) -> String {
    match license {
        "apache-2.0" => "Apache-2.0".into(),
        "mit" => "MIT".into(),
        other => other.into(),
    }
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "3"])
        .args(args)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl {} failed: {}",
            args.last().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn api(url: &str) -> Result<Value, String> {
    let body = curl(&[url])?;
    serde_json::from_slice(&body).map_err(|e| format!("{url}: {e}"))
}

/// Reads the header from the start of the file and refuses anything the engine
/// cannot run.
fn engine_runs(url: &str, size: u64) -> Result<(), String> {
    if size < 64 {
        return Err(format!("a file of {size} bytes is no model"));
    }
    let mut want = FIRST_BYTES.min(size);
    loop {
        let range = format!("0-{}", want - 1);
        // A server that ignores the range would send the whole file.
        let cap = want.to_string();
        let bytes = curl(&["--max-filesize", &cap, "-r", &range, url])?;
        match gguf::Header::parse(&bytes) {
            Ok(header) => {
                header.check_extents(size).map_err(|e| e.to_string())?;
                let config = infer::Config::from_gguf(&header).map_err(|e| e.to_string())?;
                infer::check(&header, &config).map_err(|e| e.to_string())?;
                let tok = bpe::Tokenizer::from_gguf(&header).map_err(|e| e.to_string())?;
                bpe::chat::Chat::new(&tok).map_err(|e| e.to_string())?;
                if tok.len() != config.vocab {
                    return Err("the vocabulary and the embeddings differ in size".into());
                }
                println!(
                    "xtask: the engine can run it: {} layers, {} wide, {} tokens",
                    config.n_layer, config.n_embd, config.vocab
                );
                return Ok(());
            }
            Err(e) if e.is_short() && want < MOST_BYTES.min(size) => want = (want * 2).min(size),
            Err(e) => return Err(format!("refused: {e}")),
        }
    }
}

/// Adds or replaces `entry` in the catalog file.
fn write(entry: &Entry) -> Result<(), String> {
    let path = crate::workspace_root().join(CATALOG_PATH);
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {CATALOG_PATH}: {e}"))?;
    let mut root: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let models = root["models"]
        .as_array_mut()
        .ok_or(format!("{CATALOG_PATH} has no models list"))?;
    // A re-pinned entry keeps its place, name and note.
    let mut new = entry.to_json();
    match models.iter_mut().find(|m| m["id"] == entry.id.as_str()) {
        Some(old) => {
            new["name"] = old["name"].clone();
            new["note"] = old["note"].clone();
            *old = new;
        }
        None => models.push(new),
    }
    let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())? + "\n";
    super::catalog::Catalog::parse(&text)?;
    fs::write(&path, text).map_err(|e| format!("cannot write {CATALOG_PATH}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_comes_from_the_hubs_answers() {
        // The Hub's answers for the pinned Qwen3-0.6B, trimmed to what pin reads.
        let info = serde_json::json!({"cardData": {"license": "apache-2.0"}});
        let tree = serde_json::json!([
            {"path": "README.md", "size": 6236},
            {"path": "Qwen3-0.6B-Q8_0.gguf", "size": 639_446_688, "lfs": {
                "oid": "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031",
                "size": 639_446_688
            }}
        ]);
        let commit = "23749fefcc72300e3a2ad315e1317431b06b590a";
        let file = "Qwen3-0.6B-Q8_0.gguf";
        let plain = ModelArgs {
            min_ram: Some(4),
            ..ModelArgs::default()
        };
        let entry = entry_from("Qwen/Qwen3-0.6B-GGUF", file, commit, &info, &tree, &plain).unwrap();
        assert_eq!(entry.id, "qwen3-0.6b-q8-0");
        assert_eq!(entry.name, "Qwen3-0.6B Q8_0");
        assert_eq!(entry.license, "Apache-2.0");
        let pinned = super::super::catalog::Catalog::load().unwrap();
        let committed = pinned.get("qwen3-0.6b").unwrap();
        let named = ModelArgs {
            id: Some("qwen3-0.6b".into()),
            min_ram: Some(4),
            ..ModelArgs::default()
        };
        let entry = entry_from("Qwen/Qwen3-0.6B-GGUF", file, commit, &info, &tree, &named).unwrap();
        assert_eq!(
            (
                entry.id.as_str(),
                entry.size,
                entry.sha256.as_str(),
                entry.revision.as_str()
            ),
            (
                committed.id.as_str(),
                committed.size,
                committed.sha256.as_str(),
                committed.revision.as_str()
            )
        );
        let missing = entry_from(
            "Qwen/Qwen3-0.6B-GGUF",
            "x.gguf",
            commit,
            &info,
            &tree,
            &plain,
        );
        assert!(missing.unwrap_err().contains("has no x.gguf"));
        let no_ram = ModelArgs::default();
        let no_ram = entry_from("Qwen/Qwen3-0.6B-GGUF", file, commit, &info, &tree, &no_ram);
        assert!(no_ram.unwrap_err().contains("--min-ram"));
        let other = serde_json::json!({"cardData": {"license": "llama3"}});
        let refused = entry_from("Qwen/Qwen3-0.6B-GGUF", file, commit, &other, &tree, &plain);
        assert!(refused.unwrap_err().contains("Apache-2.0 or MIT"));
    }

    #[test]
    fn licences_are_spelled_as_spdx() {
        assert_eq!(spdx("apache-2.0"), "Apache-2.0");
        assert_eq!(spdx("mit"), "MIT");
        assert_eq!(spdx("other"), "other");
        assert!(plain_name("Qwen3-0.6B-Q8_0.gguf"));
    }
}
