//! `cargo xtask model check`: is the stored file the pinned one, does the engine
//! accept it, and does it agree with llama.cpp? The llama.cpp ids come from the
//! manual `llama.cpp goldens` workflow, committed as `xtask/goldens/<id>.json`;
//! until then the check writes our own ids to `target/model-check/<id>.json`,
//! where the workflow's JSON can be compared by hand.

use super::{Loaded, ModelArgs, catalog::Catalog, fetch, store};
use infer::{Next, generate};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

const INPUTS: &str = include_str!("../../goldens/inputs.json");

/// Our answers to the golden inputs, in the workflow's JSON shape.
#[derive(Debug, PartialEq)]
pub struct Answers {
    pub tokenize: Vec<Vec<u32>>,
    pub prompt_ids: Vec<Vec<u32>>,
    pub greedy: Vec<Vec<u32>>,
}

pub fn run(args: &ModelArgs) -> Result<(), String> {
    let loaded = if args.fixture.is_some() || args.file.is_some() {
        Loaded::open(args)?
    } else {
        let catalog = Catalog::load()?;
        let entry = args.entry(&catalog, true)?;
        let path = store::resolve(args.store.as_deref())?.join(&entry.file);
        if !path.exists() {
            return Err(format!(
                "{} is not there; run cargo xtask setup {} first",
                path.display(),
                entry.id
            ));
        }
        println!("xtask: checking {} against the catalog", path.display());
        fetch::verify(&path, entry)?;
        println!("xtask: size and SHA-256 match the pinned file");
        let args = ModelArgs {
            local: Some(entry.id.clone()),
            store: args.store.clone(),
            ..ModelArgs::default()
        };
        Loaded::open(&args)?
    };
    let answers = answer(&loaded, args)?;
    let Some(entry) = &loaded.entry else {
        println!(
            "xtask: {} passes; llama.cpp goldens exist only for catalog models",
            loaded.label
        );
        return Ok(());
    };
    let name = format!("{}.json", entry.id);
    let golden_path = crate::workspace_root()
        .join("xtask")
        .join("goldens")
        .join(&name);
    match fs::read_to_string(&golden_path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("cannot read {}: {e}", golden_path.display()))
        }
        Ok(text) => {
            let golden: Value = serde_json::from_str(&text)
                .map_err(|e| format!("{}: {e}", golden_path.display()))?;
            if golden["sha256"]
                .as_str()
                .is_some_and(|sha| sha != entry.sha256)
            {
                return Err(format!(
                    "{} is for another file than the catalog pins; run the workflow again",
                    golden_path.display()
                ));
            }
            compare(&answers, &golden)
        }
        Err(_) => {
            let out = crate::workspace_root()
                .join("target")
                .join("model-check")
                .join(&name);
            write_json(&out, &to_json(&answers))?;
            println!(
                "xtask: no llama.cpp goldens yet; our ids are in {}. Run the \
                 'llama.cpp goldens' workflow on GitHub, then commit its goldens.json as {}.",
                out.display(),
                golden_path.display()
            );
            Ok(())
        }
    }
}

/// Our tokenizer's ids for every string, and our prompt and greedy ids for every
/// prompt. Every string must also decode back to itself.
pub fn answer(loaded: &Loaded, args: &ModelArgs) -> Result<Answers, String> {
    let inputs: Value = serde_json::from_str(INPUTS).map_err(|e| e.to_string())?;
    let strings = texts(&inputs["strings"]);
    let prompts = texts(&inputs["prompts"]);
    let max_new = inputs["greedy_tokens"].as_u64().unwrap_or(32) as usize;
    let file = loaded.gguf()?;
    let (tok, chat) = loaded.tokenizer()?;
    let mut tokenize = Vec::new();
    for text in &strings {
        let mut ids = Vec::new();
        tok.encode(text, &mut ids);
        let mut back = Vec::new();
        tok.decode(&ids, &mut back);
        if back != text.as_bytes() {
            return Err(format!("{text:?} does not decode back to itself"));
        }
        tokenize.push(ids);
    }
    println!(
        "xtask: {} strings encode and decode back exactly",
        strings.len()
    );
    let mut model = infer::Model::new(&file, args.tier()?, 512).map_err(|e| e.to_string())?;
    let (mut prompt_ids, mut greedy) = (Vec::new(), Vec::new());
    for question in &prompts {
        let prompt = chat.prompt(&tok, None, question);
        let mut kv = model.kv_cache().map_err(|e| e.to_string())?;
        let mut ids = Vec::new();
        generate(&mut model, &mut kv, &prompt, max_new, |t| {
            if chat.is_stop(t) {
                return Next::Stop;
            }
            ids.push(t);
            Next::Continue
        })
        .map_err(|e| e.to_string())?;
        prompt_ids.push(prompt);
        greedy.push(ids);
    }
    Ok(Answers {
        tokenize,
        prompt_ids,
        greedy,
    })
}

fn texts(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn to_json(answers: &Answers) -> Value {
    let prompts: Vec<Value> = answers
        .prompt_ids
        .iter()
        .zip(&answers.greedy)
        .map(|(p, g)| json!({"prompt_ids": p, "greedy": g}))
        .collect();
    json!({"tokenize": answers.tokenize, "prompts": prompts})
}

fn ids(value: &Value) -> Vec<u32> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_u64())
                .map(|v| v as u32)
                .collect()
        })
        .unwrap_or_default()
}

/// Compares our answers with llama.cpp's; greedy runs are compared up to the
/// first end token on either side.
pub fn compare(answers: &Answers, golden: &Value) -> Result<(), String> {
    let mut problems = Vec::new();
    let golden_tokens = golden["tokenize"].as_array().cloned().unwrap_or_default();
    if golden_tokens.len() != answers.tokenize.len() {
        problems.push(format!(
            "the goldens tokenize {} strings, the inputs have {}",
            golden_tokens.len(),
            answers.tokenize.len()
        ));
    }
    let same = answers
        .tokenize
        .iter()
        .zip(&golden_tokens)
        .enumerate()
        .filter(|(i, (ours, theirs))| {
            let ok = **ours == ids(theirs);
            if !ok {
                problems.push(format!(
                    "string {i}: ours {ours:?}, llama.cpp {:?}",
                    ids(theirs)
                ));
            }
            ok
        })
        .count();
    println!(
        "xtask: tokenizer: {same} of {} strings match llama.cpp",
        answers.tokenize.len()
    );
    let golden_prompts = golden["prompts"].as_array().cloned().unwrap_or_default();
    if golden_prompts.len() != answers.prompt_ids.len() {
        problems.push(format!(
            "the goldens have {} prompts, the inputs {}",
            golden_prompts.len(),
            answers.prompt_ids.len()
        ));
    }
    let mut greedy_same = 0;
    for (i, theirs) in golden_prompts.iter().enumerate() {
        let (Some(prompt), Some(ours)) = (answers.prompt_ids.get(i), answers.greedy.get(i)) else {
            problems.push(format!(
                "prompt {i} is in the goldens but not in the inputs"
            ));
            continue;
        };
        if *prompt != ids(&theirs["prompt_ids"]) {
            problems.push(format!(
                "prompt {i}: the ChatML prompt tokenizes differently"
            ));
        }
        let theirs = ids(&theirs["greedy"]);
        let n = ours.len().min(theirs.len());
        if ours[..n] == theirs[..n] && ours.len() == theirs.len() {
            greedy_same += 1;
        } else {
            let at = (0..n).find(|&k| ours[k] != theirs[k]).unwrap_or(n);
            problems.push(format!("prompt {i}: greedy ids differ from token {at}"));
        }
    }
    println!(
        "xtask: greedy: {greedy_same} of {} prompts match llama.cpp",
        golden_prompts.len()
    );
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "model check found differences:\n  {}",
            problems.join("\n  ")
        ))
    }
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    fs::write(path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_answers() -> Answers {
        let args = ModelArgs {
            fixture: Some("tiny".into()),
            ..ModelArgs::default()
        };
        answer(&Loaded::open(&args).unwrap(), &args).unwrap()
    }

    #[test]
    fn the_inputs_hold_three_prompts_and_fifty_strings() {
        let inputs: Value = serde_json::from_str(INPUTS).unwrap();
        assert_eq!(texts(&inputs["prompts"]).len(), 3);
        assert_eq!(texts(&inputs["strings"]).len(), 50);
        assert_eq!(inputs["greedy_tokens"], 32);
    }

    #[test]
    fn answers_compare_with_goldens() {
        let ours = fixture_answers();
        assert_eq!(ours.tokenize.len(), 50);
        assert_eq!(ours.greedy.len(), 3);
        let golden = to_json(&ours);
        assert_eq!(compare(&ours, &golden), Ok(()));
        let mut changed = golden.clone();
        changed["tokenize"][3][0] = json!(9999);
        changed["prompts"][1]["greedy"] = json!([1, 2, 3]);
        let error = compare(&ours, &changed).unwrap_err();
        assert!(
            error.contains("string 3") && error.contains("prompt 1"),
            "{error}"
        );
        // Goldens with fewer prompts, or none, are not a pass.
        let mut fewer = golden.clone();
        fewer["prompts"].as_array_mut().unwrap().pop();
        assert!(compare(&ours, &fewer).unwrap_err().contains("2 prompts"));
        let mut none = golden;
        none.as_object_mut().unwrap().remove("prompts");
        assert!(compare(&ours, &none).is_err());
    }
}
