//! The local LLM on the host, run through Kernel AI's own engine crates:
//!
//! cargo xtask setup [ID] [--store DIR] [--yes]
//! cargo xtask model chat [--local ID | --file PATH | --fixture tiny|0.6b] [--tier best|baseline] [--max N] [--system TEXT] QUESTION
//! cargo xtask model check [ID] [--store DIR]
//! cargo xtask model pin REPO FILE --min-ram GB [--revision REV] [--id ID] [--write]
//! cargo xtask bench quant
//!
//! A catalog id is named with `--local ID` (or alone, for `setup` and `check`),
//! never with `--model`, which keeps meaning the model name sent to an LLM server.

mod bench;
mod catalog;
mod chat;
mod check;
mod fetch;
mod machine;
mod pin;
mod setup;
mod store;

use catalog::{Catalog, Entry};
use std::fs;
use std::path::PathBuf;

const USAGE: &str = "expected setup, model chat|check|pin or bench quant";

pub fn main(args: &[String]) -> Result<(), String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["setup", rest @ ..] => setup::run(&ModelArgs::parse(rest)?),
        ["model", "chat", rest @ ..] => chat::run(&ModelArgs::parse(rest)?),
        ["model", "check", rest @ ..] => check::run(&ModelArgs::parse(rest)?),
        ["model", "pin", rest @ ..] => pin::run(&ModelArgs::parse(rest)?),
        ["bench", "quant"] => bench::run(),
        _ => Err(USAGE.into()),
    }
}

/// Flags shared by the model commands; each command uses the ones it needs.
#[derive(Debug, Default, PartialEq)]
pub struct ModelArgs {
    /// A catalog id.
    pub local: Option<String>,
    pub file: Option<PathBuf>,
    pub fixture: Option<String>,
    pub store: Option<String>,
    pub tier: Option<String>,
    pub max: Option<usize>,
    pub system: Option<String>,
    pub revision: Option<String>,
    /// The id `pin` gives a new catalog entry.
    pub id: Option<String>,
    pub min_ram: Option<u64>,
    pub yes: bool,
    pub write: bool,
    /// Everything that is not a flag, in order.
    pub words: Vec<String>,
}

impl ModelArgs {
    pub fn parse(args: &[&str]) -> Result<Self, String> {
        let mut out = Self::default();
        let mut rest = args.iter();
        while let Some(&arg) = rest.next() {
            let mut value = |name: &str| {
                rest.next()
                    .map(|v| v.to_string())
                    .ok_or(format!("{name} expects a value"))
            };
            match arg {
                "--local" => out.local = Some(value(arg)?),
                "--file" => out.file = Some(value(arg)?.into()),
                "--fixture" => out.fixture = Some(value(arg)?),
                "--store" => out.store = Some(value(arg)?),
                "--tier" => out.tier = Some(value(arg)?),
                "--system" => out.system = Some(value(arg)?),
                "--revision" => out.revision = Some(value(arg)?),
                "--id" => out.id = Some(value(arg)?),
                "--max" => out.max = Some(number(arg, &value(arg)?)?),
                "--min-ram" => out.min_ram = Some(number(arg, &value(arg)?)?),
                "--yes" => out.yes = true,
                "--write" => out.write = true,
                "--model" => {
                    return Err(
                        "--model names a model on an LLM server; use --local ID for a \
                                catalog model"
                            .into(),
                    );
                }
                flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
                word => out.words.push(word.to_string()),
            }
        }
        Ok(out)
    }

    pub fn tier(&self) -> Result<quant::Tier, String> {
        chat::tier(self.tier.as_deref())
    }

    /// The catalog entry named by `--local`, the first word, or the default.
    pub fn entry<'c>(&self, catalog: &'c Catalog, positional: bool) -> Result<&'c Entry, String> {
        let named = self.local.as_deref().or_else(|| {
            positional
                .then(|| self.words.first().map(String::as_str))
                .flatten()
        });
        catalog.get(named.unwrap_or(&catalog.default))
    }
}

fn number<T: std::str::FromStr>(flag: &str, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{flag} expects a number, got {text:?}"))
}

/// A model file read into memory, with where it came from.
pub struct Loaded {
    pub label: String,
    pub entry: Option<Entry>,
    bytes: Vec<u8>,
}

impl Loaded {
    /// The fixture, the file, or the catalog model the arguments name.
    pub fn open(args: &ModelArgs) -> Result<Self, String> {
        if let Some(name) = &args.fixture {
            let spec = fixture_spec(name)?;
            return Ok(Self {
                label: format!("the generated {name} fixture (random weights)"),
                entry: None,
                bytes: infer::fixture::build(&spec),
            });
        }
        if let Some(path) = &args.file {
            let bytes =
                fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            return Ok(Self {
                label: path.display().to_string(),
                entry: None,
                bytes,
            });
        }
        let catalog = Catalog::load()?;
        let entry = args.entry(&catalog, false)?.clone();
        let path = store::resolve(args.store.as_deref())?.join(&entry.file);
        let bytes = fs::read(&path).map_err(|e| {
            format!(
                "cannot read {} ({e}); run cargo xtask setup {} first",
                path.display(),
                entry.id
            )
        })?;
        if bytes.len() as u64 != entry.size {
            return Err(format!(
                "{} is not the pinned file; run cargo xtask model check {}",
                path.display(),
                entry.id
            ));
        }
        Ok(Self {
            label: entry.name.clone(),
            entry: Some(entry),
            bytes,
        })
    }

    pub fn gguf(&self) -> Result<gguf::Gguf<'_>, String> {
        gguf::Gguf::parse(&self.bytes).map_err(|e| format!("{}: {e}", self.label))
    }

    pub fn tokenizer(&self) -> Result<(bpe::Tokenizer, bpe::chat::Chat), String> {
        let file = self.gguf()?;
        let tok = bpe::Tokenizer::from_gguf(file.header()).map_err(|e| e.to_string())?;
        let chat = bpe::chat::Chat::new(&tok).map_err(|e| e.to_string())?;
        Ok((tok, chat))
    }
}

/// The stored file of catalog model `id`, for `run --local`: in the model store
/// (`--store`, else `KAI_MODELS`, else the default, as for `setup`), and of the
/// pinned size.
pub fn stored(id: &str, store: Option<&str>) -> Result<PathBuf, String> {
    let catalog = Catalog::load()?;
    let entry = catalog.get(id)?;
    let path = store::resolve(store)?.join(&entry.file);
    // The fix names the same store, so it fixes this store.
    let in_store = store.map_or_else(String::new, |dir| format!(" --store {dir}"));
    let len = fs::metadata(&path)
        .map_err(|e| {
            format!(
                "{} is not there ({e}); run cargo xtask setup {}{in_store} first",
                path.display(),
                entry.id
            )
        })?
        .len();
    if len != entry.size {
        return Err(format!(
            "{} is {len} bytes, not the pinned {}; run cargo xtask model check {}{in_store}",
            path.display(),
            entry.size,
            entry.id
        ));
    }
    println!("xtask: {} from the catalog, {}", entry.name, path.display());
    Ok(path)
}

pub fn fixture_spec(name: &str) -> Result<infer::fixture::Spec, String> {
    match name {
        "tiny" => Ok(infer::fixture::Spec::tiny()),
        "0.6b" => Ok(infer::fixture::Spec::qwen3_0_6b()),
        other => Err(format!("--fixture expects tiny or 0.6b, got {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_parse_and_model_is_refused() {
        let args =
            ModelArgs::parse(&["--local", "qwen3-0.6b", "--max", "9", "hi", "there"]).unwrap();
        assert_eq!(args.local.as_deref(), Some("qwen3-0.6b"));
        assert_eq!(args.max, Some(9));
        assert_eq!(args.words, ["hi", "there"]);
        let error = ModelArgs::parse(&["--model", "qwen3-0.6b"]).unwrap_err();
        assert!(error.contains("--local"), "{error}");
        assert!(ModelArgs::parse(&["--max", "lots"]).is_err());
        assert!(ModelArgs::parse(&["--store"]).is_err());
        assert!(ModelArgs::parse(&["--fast"]).is_err());
        assert!(main(&["model".into(), "talk".into()]).is_err());
    }

    #[test]
    fn run_local_needs_a_known_id_and_the_stored_file() {
        let unknown = stored("qwen3-9b", None).unwrap_err();
        assert!(unknown.contains("no model"), "{unknown}");
        // The store on a dev PC or a CI runner holds no model; the message says
        // how to get it (or the store is refused, on a PC whose temp is on C:).
        if let Err(missing) = stored("qwen3-0.6b", None) {
            assert!(
                missing.contains("cargo xtask setup qwen3-0.6b")
                    || missing.contains("model check")
                    || missing.contains("C:")
                    || missing.contains("OneDrive"),
                "{missing}"
            );
        }
    }

    #[test]
    fn the_entry_comes_from_local_then_the_first_word() {
        let catalog = Catalog::load().unwrap();
        let by_word = ModelArgs::parse(&["qwen3-0.6b"]).unwrap();
        assert_eq!(by_word.entry(&catalog, true).unwrap().id, "qwen3-0.6b");
        let unknown = ModelArgs::parse(&["qwen3-9b"]).unwrap();
        assert!(unknown.entry(&catalog, true).is_err());
        // A question's first word is not an id.
        assert_eq!(unknown.entry(&catalog, false).unwrap().id, catalog.default);
    }
}
