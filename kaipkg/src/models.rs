//! The image's signed `models` role (ADR 0008 §6 and §9; P4 task 10): the
//! models a user may import, each a TUF target `<id>.gguf` with its length,
//! SHA-256 and `custom.kai` fields `name`, `licence` and `source`. The image
//! carries the role's metadata and no weights (P4 plan §4, decision 4): the
//! user attaches the GGUF as a disk, and the importing service holds it to the
//! target's length and hash before it is stored.
//!
//! Metadata that does not verify refuses the whole role, as for packages
//! ([`crate::image`]). Each target is then read on its own and one that is
//! malformed is refused by path; the rest are kept. The fields are data the
//! model manager shows, never instructions, so each is short plain text with
//! nothing that reorders or hides the text around it, as a catalogue summary
//! is drawn. The licence must have an SPDX expression's shape, as a package
//! header's must, but which licences are taken is not judged here: the import
//! refuses one the project does not take, and says so.
//!
//! From P5 the role also signs each downloadable model's `kai-chunks 1` list
//! beside it, the target `<sha256 of the GGUF>.chunks` (P5 tasks 6 and 9). A
//! list is not a model to offer: [`check`] sets each apart, by its model's
//! hash, for the download that fetches and parses it, and refuses one whose
//! name is not a model's hash or that names no model of the role.

use crate::Sha;
use crate::catalog::reorders;
use crate::header::valid_licence;
use crate::image::{self, Files};
use crate::index::sha256;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kaituf::meta::TargetFile;
use serde_json::Value;

/// The role every model sits under, delegated by `targets` for `*.gguf`.
pub const ROLE: &str = "models";
/// A target's path is its id and this.
pub const EXTENSION: &str = ".gguf";
/// A model's chunk list is the target `<sha256>.chunks`, named by the GGUF's
/// hash in lower-case hex, so it pairs with that model alone.
pub const CHUNKS_EXTENSION: &str = ".chunks";

/// The target path of the chunk list for the model whose GGUF has `sha256`.
pub fn chunks_path(sha256: &Sha) -> String {
    format!("{}{CHUNKS_EXTENSION}", crate::hex(sha256))
}

/// The SHA-256 the role signs for `target`, such as a chunk list of
/// [`Checked::chunks`], which a fetched copy must hash to.
pub fn target_sha256(target: &TargetFile) -> Result<Sha, String> {
    sha256(target)
}
/// `custom.kai`'s fields, all required.
const FIELDS: [&str; 3] = ["name", "licence", "source"];
/// Longest id, in bytes.
pub const MAX_ID: usize = 64;
/// Longest field, in bytes.
pub const MAX_TEXT: usize = 256;

/// One model the role signs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// The target's path without [`EXTENSION`], such as `qwen3-4b-2507`.
    pub id: String,
    pub name: String,
    pub length: u64,
    pub sha256: Sha,
    /// An SPDX id.
    pub licence: String,
    /// Where it comes from: the Hugging Face URL at the pinned revision.
    pub source: String,
}

/// The role's models that read, and the targets refused, with why.
#[derive(Debug, Default)]
pub struct Checked {
    pub models: Vec<Model>,
    pub refused: Vec<(String, String)>,
    /// Each model's chunk list the role signs: the GGUF's SHA-256 and the
    /// list's target, whose length and hash the fetched list is held to.
    pub chunks: Vec<(Sha, TargetFile)>,
}

/// The `models` role in the image's repository `files`. An error means the
/// metadata did not verify, and no model is kept.
pub fn check(files: &mut dyn Files) -> Result<Checked, String> {
    let image::Listed { targets, refused } = image::listed(files, ROLE)?;
    Ok(sort(targets, refused))
}

/// The role's verified `targets` as models and chunk lists, each malformed
/// one added to `refused` by path.
fn sort(targets: Vec<TargetFile>, refused: Vec<(String, String)>) -> Checked {
    let mut checked = Checked {
        refused,
        ..Checked::default()
    };
    let (lists, models): (Vec<_>, Vec<_>) = targets
        .into_iter()
        .partition(|t| t.path.ends_with(CHUNKS_EXTENSION));
    for target in models {
        match parse(&target) {
            Ok(model) => checked.models.push(model),
            Err(why) => checked.refused.push((target.path, why)),
        }
    }
    for list in lists {
        match list_of(&list, &checked.models) {
            Ok(sha) => checked.chunks.push((sha, list)),
            Err(why) => checked.refused.push((list.path, why)),
        }
    }
    checked
}

/// The SHA-256 of the model `list` is the chunk list of, among `models`.
fn list_of(list: &TargetFile, models: &[Model]) -> Result<Sha, String> {
    let sha = list
        .path
        .strip_suffix(CHUNKS_EXTENSION)
        .and_then(crate::unhex)
        .filter(|sha| chunks_path(sha) == list.path)
        .ok_or("its path is not a model's SHA-256 in lower-case hex and .chunks")?;
    if list.length == 0 || sha256(list).is_err() {
        return Err("it has no length or no SHA-256".into());
    }
    match models.iter().any(|m| m.sha256 == sha) {
        true => Ok(sha),
        false => Err("it names no model the role signs".into()),
    }
}

/// `target` as a model, parsed strictly.
pub fn parse(target: &TargetFile) -> Result<Model, String> {
    let id = target
        .path
        .strip_suffix(EXTENSION)
        .filter(|id| plain_id(id))
        .ok_or_else(|| {
            format!("its path is not an id of at most {MAX_ID} bytes [a-z0-9._-] and {EXTENSION}")
        })?;
    if target.length == 0 {
        return Err("its length is 0".into());
    }
    let kai = target
        .custom
        .get("kai")
        .and_then(Value::as_object)
        .ok_or("it has no custom.kai")?;
    if let Some(key) = kai.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(format!("custom.kai has an unknown field {key}"));
    }
    let text = |key: &str| -> Result<String, String> {
        let value = kai
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("custom.kai.{key} is not a string"))?;
        let hidden = |c: char| c.is_control() || reorders(c);
        if value.is_empty() || value.len() > MAX_TEXT || value.chars().any(hidden) {
            return Err(format!(
                "custom.kai.{key} is not 1 to {MAX_TEXT} bytes of plain text"
            ));
        }
        Ok(value.into())
    };
    let licence = text("licence")?;
    if !valid_licence(&licence) {
        return Err("custom.kai.licence is not an SPDX expression".into());
    }
    Ok(Model {
        id: id.into(),
        name: text("name")?,
        length: target.length,
        sha256: sha256(target)?,
        licence,
        source: text("source")?,
    })
}

/// An id a path, a command and a list cell all take as it is.
fn plain_id(id: &str) -> bool {
    let ok = |b: u8| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-');
    !id.is_empty() && id.len() <= MAX_ID && !id.starts_with('.') && id.bytes().all(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(path: &str, kai: Value) -> TargetFile {
        TargetFile {
            path: path.into(),
            length: 4096,
            hashes: alloc::vec![("sha256".into(), alloc::vec![0xab; 32])],
            custom: json!({ "kai": kai }),
        }
    }

    fn fields() -> Value {
        json!({
            "name": "Qwen3-4B-Instruct-2507 Q4_K_M",
            "licence": "Apache-2.0",
            "source": "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/a06e946/m.gguf",
        })
    }

    /// A chunk list's target at `path`.
    fn list(path: String) -> TargetFile {
        TargetFile {
            path,
            length: 77_000,
            hashes: alloc::vec![("sha256".into(), alloc::vec![0xcd; 32])],
            custom: json!({}),
        }
    }

    #[test]
    fn a_role_with_a_chunk_list_offers_only_the_model_and_keeps_the_list_apart() {
        let model = target("qwen3-4b-2507.gguf", fields());
        let beside = list(chunks_path(&[0xab; 32]));
        assert_eq!(beside.path, alloc::format!("{}.chunks", "ab".repeat(32)));
        let checked = sort(alloc::vec![beside.clone(), model], Vec::new());
        let ids: Vec<&str> = checked.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["qwen3-4b-2507"]);
        assert_eq!(checked.refused, []);
        assert_eq!(checked.chunks, [([0xab; 32], beside)]);
    }

    #[test]
    fn a_chunk_list_that_names_no_model_or_is_misnamed_is_refused_by_path() {
        let orphan = list(chunks_path(&[0x12; 32]));
        let upper = list(alloc::format!("{}.chunks", "AB".repeat(32)));
        let short = list("abc.chunks".into());
        let empty = TargetFile {
            length: 0,
            ..list(chunks_path(&[0xab; 32]))
        };
        let targets = alloc::vec![target("m.gguf", fields()), orphan, upper, short, empty];
        let checked = sort(targets, Vec::new());
        assert_eq!(checked.models.len(), 1);
        assert!(checked.chunks.is_empty());
        let why: Vec<&str> = checked.refused.iter().map(|(_, w)| w.as_str()).collect();
        assert_eq!(
            why,
            [
                "it names no model the role signs",
                "its path is not a model's SHA-256 in lower-case hex and .chunks",
                "its path is not a model's SHA-256 in lower-case hex and .chunks",
                "it has no length or no SHA-256",
            ]
        );
    }

    #[test]
    fn a_target_reads_as_its_model() {
        let model = parse(&target("qwen3-4b-2507.gguf", fields())).unwrap();
        assert_eq!(model.id, "qwen3-4b-2507");
        assert_eq!(model.name, "Qwen3-4B-Instruct-2507 Q4_K_M");
        assert_eq!((model.length, model.sha256), (4096, [0xab; 32]));
        assert_eq!(model.licence, "Apache-2.0");
        assert!(model.source.starts_with("https://huggingface.co/unsloth/"));
    }

    #[test]
    fn a_malformed_target_is_refused_with_why() {
        let with = |key: &str, value: Value| {
            let mut kai = fields();
            kai[key] = value;
            parse(&target("m.gguf", kai)).unwrap_err()
        };
        assert!(with("name", json!(7)).contains("not a string"));
        assert!(with("name", json!("")).contains("plain text"));
        assert!(with("source", json!("a\nb")).contains("plain text"));
        for hidden in ['\u{202E}', '\u{200B}', '\u{2066}', '\u{FEFF}', '\u{061C}'] {
            let name = format!("Qwen{hidden}3");
            assert!(
                with("name", json!(name)).contains("plain text"),
                "{hidden:?}"
            );
        }
        assert!(with("licence", json!("MIT; rm -rf")).contains("SPDX"));
        assert!(with("licence", json!("MIT/X11")).contains("SPDX"));
        let ok = parse(&target("m.gguf", {
            let mut kai = fields();
            kai["licence"] = json!("Apache-2.0 OR MIT");
            kai
        }));
        assert_eq!(ok.unwrap().licence, "Apache-2.0 OR MIT");
        let long = "x".repeat(MAX_TEXT + 1);
        assert!(with("licence", json!(long)).contains("plain text"));
        assert!(with("extra", json!("x")).contains("unknown field extra"));
        let mut missing = fields();
        missing.as_object_mut().unwrap().remove("licence");
        assert!(parse(&target("m.gguf", missing)).is_err());
        for path in [
            "m.kpkg", ".gguf", "..gguf", "M.gguf", "a/b.gguf", "a b.gguf",
        ] {
            assert!(parse(&target(path, fields())).is_err(), "{path}");
        }
        let long = format!("{}.gguf", "a".repeat(MAX_ID + 1));
        assert!(parse(&target(&long, fields())).is_err());
        let mut empty = target("m.gguf", fields());
        empty.length = 0;
        assert!(parse(&empty).unwrap_err().contains("length"));
        let mut unhashed = target("m.gguf", fields());
        unhashed.hashes = alloc::vec![("sha512".into(), alloc::vec![0; 64])];
        assert!(parse(&unhashed).unwrap_err().contains("SHA-256"));
        let mut bare = target("m.gguf", fields());
        bare.custom = Value::Null;
        assert!(parse(&bare).unwrap_err().contains("custom.kai"));
    }
}
