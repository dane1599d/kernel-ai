//! The git-pinned model catalog, `xtask/models.json`. Each entry pins a file by
//! repository commit, size and SHA-256, so a download is kept only if it is byte
//! for byte the file reviewed here, and lists what a decode step reads, so xtask
//! knows whether the engine runs it yet.

use super::decode::Decode;
use serde_json::{Value, json};

const CATALOG: &str = include_str!("../../models.json");
pub const CATALOG_PATH: &str = "xtask/models.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub repo: String,
    /// A 40-hex commit of the Hugging Face repository.
    pub revision: String,
    /// A plain file name, also its name in the store.
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub license: String,
    /// The least RAM, in GB, this model is recommended for.
    pub min_ram_gb: u64,
    pub note: String,
    /// Every matrix a decode step reads, from the file's header.
    pub decode: Decode,
}

impl Entry {
    pub fn url(&self) -> String {
        resolve_url(&self.repo, &self.revision, &self.file)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "repo": self.repo,
            "revision": self.revision,
            "file": self.file,
            "size": self.size,
            "sha256": self.sha256,
            "license": self.license,
            "min_ram_gb": self.min_ram_gb,
            "note": self.note,
            "decode": self.decode.to_json(),
        })
    }

    /// Why the engine cannot run this model yet, if it cannot: a matrix type
    /// its forward pass does not have. Such a model is pinned, and `setup ID`
    /// downloads it, but it is never recommended, the default, or run.
    pub fn not_runnable(&self) -> Option<String> {
        let types = self.decode.types_not_run();
        (!types.is_empty()).then(|| {
            format!(
                "{} has {} weights, which the engine does not run",
                self.name,
                types.join(" and ")
            )
        })
    }

    fn from_json(value: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_string)
                .ok_or(format!("a catalog entry has no {key}"))
        };
        let number = |key: &str| {
            value[key]
                .as_u64()
                .ok_or(format!("a catalog entry has no {key}"))
        };
        let entry = Self {
            id: text("id")?,
            name: text("name")?,
            repo: text("repo")?,
            revision: text("revision")?,
            file: text("file")?,
            size: number("size")?,
            sha256: text("sha256")?,
            license: text("license")?,
            min_ram_gb: number("min_ram_gb")?,
            note: value["note"].as_str().unwrap_or("").to_string(),
            decode: Decode::from_json(&value["decode"]).map_err(|e| {
                let id = value["id"].as_str().unwrap_or("?");
                format!("{id}: {e}; run model pin again")
            })?,
        };
        entry.check()?;
        Ok(entry)
    }

    /// Every field is safe to put in a URL or a path.
    pub fn check(&self) -> Result<(), String> {
        let id_ok = !self.id.is_empty()
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
        if !id_ok {
            return Err(format!(
                "catalog id {:?} is not lowercase letters, digits, . and -",
                self.id
            ));
        }
        let repo_ok = self.repo.split('/').count() == 2 && self.repo.split('/').all(plain_name);
        if !repo_ok {
            return Err(format!(
                "{}: repo {:?} is not owner/name",
                self.id, self.repo
            ));
        }
        if !is_hex(&self.revision, 40) {
            return Err(format!("{}: revision is not a 40-hex commit", self.id));
        }
        if !plain_name(&self.file) || !self.file.ends_with(".gguf") {
            return Err(format!(
                "{}: file {:?} is not a plain .gguf name",
                self.id, self.file
            ));
        }
        if !is_hex(&self.sha256, 64) {
            return Err(format!(
                "{}: sha256 is not 64 lowercase hex digits",
                self.id
            ));
        }
        if self.size == 0 || self.min_ram_gb == 0 || self.license.is_empty() {
            return Err(format!(
                "{}: size, min_ram_gb and license must be set",
                self.id
            ));
        }
        Ok(())
    }
}

pub fn resolve_url(repo: &str, revision: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{revision}/{file}")
}

/// Letters, digits, `.`, `_` and `-`, and not a dot path.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

#[derive(Debug)]
pub struct Catalog {
    pub default: String,
    pub models: Vec<Entry>,
}

impl Catalog {
    /// The catalog compiled into xtask.
    pub fn load() -> Result<Self, String> {
        Self::parse(CATALOG)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| format!("{CATALOG_PATH}: {e}"))?;
        let models = root["models"]
            .as_array()
            .ok_or(format!("{CATALOG_PATH} has no models list"))?
            .iter()
            .map(Entry::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        for (i, entry) in models.iter().enumerate() {
            if models[..i].iter().any(|e| e.id == entry.id) {
                return Err(format!("{CATALOG_PATH} lists {} twice", entry.id));
            }
        }
        let default = root["default"].as_str().unwrap_or_default().to_string();
        let catalog = Self { default, models };
        if let Some(why) = catalog.get(&catalog.default)?.not_runnable() {
            return Err(format!("{CATALOG_PATH}: the default model must run: {why}"));
        }
        Ok(catalog)
    }

    pub fn get(&self, id: &str) -> Result<&Entry, String> {
        self.models.iter().find(|e| e.id == id).ok_or_else(|| {
            let ids: Vec<&str> = self.models.iter().map(|e| e.id.as_str()).collect();
            format!("no model {id:?} in the catalog; it has {}", ids.join(", "))
        })
    }

    /// The largest model the engine runs that is recommended for this much RAM.
    pub fn recommend(&self, ram_gb: u64) -> Option<&Entry> {
        self.runnable()
            .filter(|e| e.min_ram_gb <= ram_gb)
            .max_by_key(|e| (e.min_ram_gb, e.size))
    }

    /// The models the engine runs now.
    pub fn runnable(&self) -> impl Iterator<Item = &Entry> {
        self.models.iter().filter(|e| e.not_runnable().is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_parses_and_pins_the_mvp_model() {
        let catalog = Catalog::load().unwrap();
        let entry = catalog.get("qwen3-0.6b").unwrap();
        assert_eq!(catalog.default, "qwen3-4b-2507");
        assert_eq!(entry.revision, "23749fefcc72300e3a2ad315e1317431b06b590a");
        assert_eq!(entry.size, 639_446_688);
        assert_eq!(
            entry.sha256,
            "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031"
        );
        assert_eq!(
            entry.url(),
            "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/\
             23749fefcc72300e3a2ad315e1317431b06b590a/Qwen3-0.6B-Q8_0.gguf"
        );
        // The 4B from 8 GB up, the 1.7B from 4 GB; the 0.6B only when named.
        assert_eq!(catalog.recommend(32).unwrap().id, "qwen3-4b-2507");
        assert_eq!(catalog.recommend(8).unwrap().id, "qwen3-4b-2507");
        assert_eq!(catalog.recommend(7).unwrap().id, "qwen3-1.7b");
        assert_eq!(catalog.recommend(4).unwrap().id, "qwen3-1.7b");
        assert!(catalog.runnable().any(|e| e.id == "qwen3-0.6b"));
        assert!(catalog.recommend(2).is_none());
        let round_trip = Entry::from_json(&entry.to_json()).unwrap();
        assert_eq!(&round_trip, entry);
        assert!(
            catalog
                .get("qwen3-9000b")
                .unwrap_err()
                .contains("qwen3-0.6b")
        );
    }

    #[test]
    fn unsafe_or_unpinned_entries_are_refused() {
        let good = Catalog::load().unwrap().models[0].to_json();
        let with = |key: &str, value: Value| {
            let mut entry = good.clone();
            entry[key] = value;
            Entry::from_json(&entry)
        };
        assert!(with("file", json!("../evil.gguf")).is_err());
        assert!(with("file", json!("a\\b.gguf")).is_err());
        assert!(with("file", json!("model.bin")).is_err());
        assert!(with("repo", json!("Qwen/../x")).is_err());
        assert!(with("repo", json!("Qwen")).is_err());
        assert!(with("revision", json!("main")).is_err());
        assert!(with("sha256", json!("9465E63A")).is_err());
        assert!(with("size", json!(0)).is_err());
        assert!(with("id", json!("Qwen3 0.6B")).is_err());
        assert!(with("license", Value::Null).is_err());
        assert!(with("note", Value::Null).is_ok());
        let twice = format!("{{\"default\": \"qwen3-0.6b\", \"models\": [{good}, {good}]}}");
        assert!(Catalog::parse(&twice).unwrap_err().contains("twice"));
        let no_default = format!("{{\"default\": \"x\", \"models\": [{good}]}}");
        assert!(Catalog::parse(&no_default).is_err());
    }
}
