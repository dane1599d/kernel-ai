//! `cargo xtask model chat`: one question to a model through Kernel AI's own
//! engine crates on the host, the answer streamed as it is made, then a line with
//! the time to the first token and the tokens per second.

use super::{Loaded, ModelArgs};
use infer::{Ended, Next, generate};
use quant::Tier;
use std::io::Write;
use std::time::Instant;

/// Tokens of context the chat sets aside: about 235 MB of KV cache for the 0.6B.
const CONTEXT: usize = 1024;
const MAX_NEW: usize = 256;

pub fn run(args: &ModelArgs) -> Result<(), String> {
    let question = args.words.join(" ");
    if question.trim().is_empty() {
        return Err("model chat expects a question, for example: \
                    cargo xtask model chat \"What does a kernel do?\""
            .into());
    }
    let loaded = Loaded::open(args)?;
    let tier = args.tier()?;
    let (tok, chat) = loaded.tokenizer()?;
    let file = loaded.gguf()?;
    let context = CONTEXT.min(
        infer::Config::from_gguf(file.header())
            .map_err(|e| e.to_string())?
            .context_length,
    );
    let mut model = infer::Model::new(&file, tier, context).map_err(|e| e.to_string())?;
    let mut kv = model.kv_cache().map_err(|e| e.to_string())?;
    let prompt = chat.prompt(&tok, args.system.as_deref(), &question);
    if prompt.len() >= context {
        return Err(format!(
            "the question takes {} tokens; the chat holds {context}",
            prompt.len()
        ));
    }
    let max_new = args.max.unwrap_or(MAX_NEW).min(context - prompt.len());
    println!(
        "xtask: {} on the {} tier, {} prompt tokens",
        loaded.label,
        tier.name(),
        prompt.len()
    );
    let mut out = Utf8Out::default();
    let mut stdout = std::io::stdout();
    let start = Instant::now();
    let (mut first, mut last, mut calls) = (start, start, 0u32);
    let mut made = 0usize;
    let ended = generate(&mut model, &mut kv, &prompt, max_new, |token| {
        last = Instant::now();
        if calls == 0 {
            first = last;
        }
        calls += 1;
        if chat.is_stop(token) {
            return Next::Stop;
        }
        made += 1;
        let mut bytes = Vec::new();
        tok.decode(&[token], &mut bytes);
        out.write(&bytes, &mut stdout);
        Next::Continue
    })
    .map_err(|e| e.to_string())?;
    out.finish(&mut stdout);
    // Each token after the first took one forward pass.
    let decode = (last - first).as_secs_f64();
    let rate = if calls > 1 && decode > 0.0 {
        f64::from(calls - 1) / decode
    } else {
        0.0
    };
    let first = (first - start).as_secs_f64();
    let why = match ended {
        Ended::Stopped => "",
        Ended::MaxTokens => ", stopped at the token limit",
        Ended::ContextFull => ", stopped when the context was full",
    };
    println!(
        "\nxtask: {made} tokens; first after {first:.2} s; {rate:.1} tokens/s{why} ({} tier)",
        tier.name()
    );
    Ok(())
}

/// Writes bytes as text as they come, holding back an unfinished UTF-8 character
/// until its last byte arrives; a byte that can never be UTF-8 becomes U+FFFD.
#[derive(Default)]
pub struct Utf8Out {
    pending: Vec<u8>,
}

impl Utf8Out {
    pub fn write(&mut self, bytes: &[u8], out: &mut impl Write) {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    text.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(e) => {
                    let good = e.valid_up_to();
                    text.push_str(std::str::from_utf8(&self.pending[..good]).unwrap_or_default());
                    let Some(bad) = e.error_len() else {
                        self.pending.drain(..good);
                        break;
                    };
                    text.push(char::REPLACEMENT_CHARACTER);
                    self.pending.drain(..good + bad);
                }
            }
        }
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }

    pub fn finish(&mut self, out: &mut impl Write) {
        if !self.pending.is_empty() {
            let text = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            let _ = out.write_all(text.as_bytes());
        }
    }
}

/// Which tier to run: the best the CPU has, or the baseline when asked.
pub fn tier(name: Option<&str>) -> Result<Tier, String> {
    match name {
        None | Some("best") => Ok(Tier::best()),
        Some("baseline") => Ok(Tier::BASELINE),
        Some(other) => Err(format!("--tier expects best or baseline, got {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_split_across_tokens_come_out_whole() {
        let mut out = Vec::new();
        let mut text = Utf8Out::default();
        // e-acute in two tokens, then a stray byte followed by half of a CJK char.
        text.write(b"caf\xc3", &mut out);
        assert_eq!(out, b"caf");
        text.write(b"\xa9 \xff\xe4\xb8", &mut out);
        assert_eq!(
            String::from_utf8(out.clone()).unwrap(),
            "caf\u{e9} \u{fffd}"
        );
        text.write(b"\xad!", &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "caf\u{e9} \u{fffd}\u{4e2d}!"
        );
    }
}
