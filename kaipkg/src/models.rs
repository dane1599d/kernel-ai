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
//! model manager shows, never instructions, so each is short plain text. The
//! licence is not judged here: the import refuses one the project does not
//! take, and says so.

use crate::Sha;
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
}

/// The `models` role in the image's repository `files`. An error means the
/// metadata did not verify, and no model is kept.
pub fn check(files: &mut dyn Files) -> Result<Checked, String> {
    let image::Listed { targets, refused } = image::listed(files, ROLE)?;
    let mut checked = Checked {
        refused,
        ..Checked::default()
    };
    for target in targets {
        match parse(&target) {
            Ok(model) => checked.models.push(model),
            Err(why) => checked.refused.push((target.path, why)),
        }
    }
    Ok(checked)
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
        if value.is_empty() || value.len() > MAX_TEXT || value.chars().any(char::is_control) {
            return Err(format!(
                "custom.kai.{key} is not 1 to {MAX_TEXT} bytes of plain text"
            ));
        }
        Ok(value.into())
    };
    Ok(Model {
        id: id.into(),
        name: text("name")?,
        length: target.length,
        sha256: sha256(target)?,
        licence: text("licence")?,
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
