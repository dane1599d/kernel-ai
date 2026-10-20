//! Host equals guest for `tune`: the host engine's answer to a request, decoded
//! exactly as the llm service decodes it (the guest's context cap and memory
//! budget, the answer's schema, greedy), and `cargo xtask tune check`, which
//! rebuilds every request a boot's llm service traced and requires its answer to
//! be the host's, byte for byte.
//!
//! `cargo xtask tune check --log FILE (--local ID [--store DIR] | --model PATH)`

use infer::Next;
use infer::chat::{Budget, Engine};
use llmproto::chat::Completion;
use llmproto::tune::{TuneRequest, encode_completion, replay};
use quant::Tier;
use std::path::PathBuf;

const USAGE: &str =
    "usage: cargo xtask tune check --log FILE (--local ID [--store DIR] | --model PATH)";

/// The engine's answer to `request` on every tier this host has, which must
/// agree, as they must in the guest.
pub fn host_answer(bytes: &[u8], request: &TuneRequest) -> Result<Completion, String> {
    let budget = Budget {
        kv_bytes: kabi::model::KV_BYTES as usize,
        other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize,
    };
    let mut answers: Vec<(&str, Completion)> = Vec::new();
    for tier in Tier::all() {
        let mut engine = Engine::load(bytes, tier, kabi::model::CONTEXT_CAP, budget)
            .map_err(|e| format!("the model: {e}"))?;
        let completion = llmproto::local::tune(&mut engine, request, |_| Next::Continue)
            .map_err(|e| format!("the model: {}", llmproto::local::describe(&e)))?;
        answers.push((tier.name(), completion));
    }
    let (_, first) = answers.first().cloned().ok_or("no engine tier")?;
    if let Some((tier, other)) = answers.iter().find(|(_, c)| *c != first) {
        return Err(format!("the {tier} tier answers {other:?}, not {first:?}"));
    }
    Ok(first)
}

/// Checks every traced request of `transcript` against the host; how many.
pub fn check_transcript(transcript: &str, bytes: &[u8]) -> Result<usize, String> {
    let requests = replay::untrace(transcript.lines(), "request");
    let answers = replay::untrace(transcript.lines(), "answer");
    if requests.is_empty() {
        return Err("the transcript traces no tune request".into());
    }
    for (seq, wire) in &requests {
        let request = TuneRequest::decode(wire)
            .ok_or(format!("traced request {seq} is not a tune request"))?;
        let guest = answers
            .iter()
            .find(|(s, _)| s == seq)
            .map(|(_, answer)| answer.as_str())
            .ok_or(format!("request {seq} has no traced answer"))?;
        let host = encode_completion(&host_answer(bytes, &request)?);
        if guest != host {
            return Err(format!(
                "request {seq}: the guest answered {guest:?}, the host engine {host:?}"
            ));
        }
        println!(
            "xtask: tune request {seq} (prompt {:016x}): the guest's answer is the host engine's, byte for byte: {host}",
            replay::prompt_hash(&request)
        );
    }
    Ok(requests.len())
}

pub fn main(args: &[String]) -> Result<(), String> {
    let (mut log, mut model) = (None, crate::models::ModelArgs::default());
    let mut path = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().ok_or(format!("{arg} expects a value"));
        match arg.as_str() {
            "--log" => log = Some(PathBuf::from(value()?)),
            "--local" => model.local = Some(value()?),
            "--store" => model.store = Some(value()?),
            "--model" => path = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown option {other}; {USAGE}")),
        }
    }
    let log = log.ok_or(USAGE)?;
    let transcript =
        std::fs::read_to_string(&log).map_err(|e| format!("cannot read {}: {e}", log.display()))?;
    let bytes = match path {
        Some(path) => {
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?
        }
        None if model.local.is_some() => crate::models::Loaded::open(&model)?.into_bytes(),
        None => return Err(USAGE.into()),
    };
    let checked = check_transcript(&transcript, &bytes)?;
    println!("xtask: PASS: {checked} tune answers in the guest equal the host engine's");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Vec<u8> {
        let example = llmproto::tune::example();
        let text = format!("{}\n{}", llmproto::tune::system_prompt(), example.text);
        infer::fixture::local_script(&text)
    }

    #[test]
    fn a_traced_answer_must_be_the_host_engines_byte_for_byte() {
        let bytes = model();
        let request = llmproto::tune::example();
        let answer = encode_completion(&host_answer(&bytes, &request).unwrap());
        assert!(answer.starts_with("={\"reason\":\"") || answer.starts_with("!{\"reason\":\""));
        let mut transcript: Vec<String> = replay::trace("request", 1, &request.encode());
        transcript.extend(replay::trace("answer", 1, &answer));
        assert_eq!(check_transcript(&transcript.join("\n"), &bytes), Ok(1));
        let mut forged: Vec<String> = replay::trace("request", 1, &request.encode());
        forged.extend(replay::trace("answer", 1, "={}"));
        assert!(check_transcript(&forged.join("\n"), &bytes).is_err());
        assert!(check_transcript("nothing", &bytes).is_err());
    }
}
