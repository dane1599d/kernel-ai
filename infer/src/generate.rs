//! Greedy generation: feed the prompt, then take the most likely token again and
//! again. Each new token goes to a callback first, which is where a caller stops
//! at an end token, keeps a deadline, or yields between tokens.

use crate::{Error, KvCache, Model};
use alloc::vec::Vec;

/// What the callback wants after a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Continue,
    Stop,
}

/// Why generation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The callback said stop.
    Stopped,
    /// `max_new` tokens went to the callback.
    MaxTokens,
    /// The KV cache has no room for the next token.
    ContextFull,
}

/// Feeds `prompt` after whatever `kv` already holds, then generates up to
/// `max_new` tokens, handing each to `on_token`. The prompt must hold at least
/// one token, fit in the cache and name only tokens in the vocabulary; all of
/// that is checked before anything is fed. The last token handed out (the end
/// token that stopped the answer, say) is not fed back, so it is not in `kv`: a
/// caller continuing the conversation feeds it first.
pub fn generate(
    model: &mut Model<'_>,
    kv: &mut KvCache,
    prompt: &[u32],
    max_new: usize,
    mut on_token: impl FnMut(u32) -> Next,
) -> Result<Ended, Error> {
    if prompt.is_empty() {
        return Err(Error::EmptyPrompt);
    }
    let vocab = model.config().vocab;
    if let Some(&bad) = prompt.iter().find(|&&t| t as usize >= vocab) {
        return Err(Error::Token(bad));
    }
    if !model.fits(kv) {
        return Err(Error::CacheShape);
    }
    if kv.len() + prompt.len() > kv.capacity() {
        return Err(Error::ContextFull);
    }
    let mut logits = Vec::new();
    logits
        .try_reserve_exact(vocab)
        .map_err(|_| Error::OutOfMemory)?;
    logits.resize(vocab, 0.0f32);
    // In blocks, with the bits of feeding the tokens one at a time.
    model.forward_batch(kv, prompt, Some(&mut logits))?;
    for made in 1..=max_new {
        let token = argmax(&logits);
        if on_token(token) == Next::Stop {
            return Ok(Ended::Stopped);
        }
        if made == max_new {
            break;
        }
        if kv.len() >= kv.capacity() {
            return Ok(Ended::ContextFull);
        }
        model.forward(kv, token, Some(&mut logits))?;
    }
    Ok(Ended::MaxTokens)
}

/// The index of the largest logit, the lowest index among equals, exactly as
/// llama.cpp's greedy sampler picks: it starts from index 0 and moves only to a
/// strictly larger logit, so a NaN never wins, and a NaN at index 0 keeps 0.
pub fn argmax(logits: &[f32]) -> u32 {
    let mut best = 0;
    for (i, &v) in logits.iter().enumerate().skip(1) {
        if v > logits[best] {
            best = i;
        }
    }
    best as u32
}
