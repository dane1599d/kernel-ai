//! One answer from a model file, run the same way by the llm service inside Kernel
//! AI and by xtask on the host: the ChatML prompt with thinking off, then greedy
//! decoding until an end token, the token limit or the end of the context. A
//! callback after every step, prompt tokens included, is where the caller sleeps,
//! keeps a deadline or stops.
//!
//! [`Engine::load`] treats the file as hostile: before it allocates anything sized
//! by the file it estimates what the tokenizer, the logits and the RoPE table will
//! take and refuses a file that would not fit [`Budget::other_bytes`], so an
//! oversized vocabulary fails with an error instead of exhausting the heap. The KV
//! cache then gets as many positions as [`Budget::kv_bytes`] holds, at most the
//! context cap and the model's trained context.

use crate::{Config, Error, KvCache, Model, Next, argmax};
use alloc::string::String;
use alloc::vec::Vec;
use bpe::Tokenizer;
use bpe::chat::Chat;
use gguf::Gguf;
use quant::Tier;

/// The fewest positions worth running: a short question and a short answer.
pub const MIN_CONTEXT: usize = 32;
/// Room for the allocator and the small buffers the estimate leaves out.
const SLACK_BYTES: usize = 1 << 20;

/// What the engine may take of its process's memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// The KV cache: two f32 vectors per layer and position.
    pub kv_bytes: usize,
    /// Everything else sized by the file: the tokenizer while it is built, the
    /// logits, the RoPE table and the scratch vectors.
    pub other_bytes: usize,
}

/// Why a model file cannot be run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    File(gguf::Error),
    Tokenizer(bpe::Error),
    Model(Error),
    /// The tokenizer, logits and tables would take this many bytes, more than the
    /// budget allows.
    TooLarge(usize),
    /// The KV budget holds fewer than [`MIN_CONTEXT`] positions of this model.
    NoContext(usize),
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::File(error) => write!(f, "the file: {error}"),
            Self::Tokenizer(error) => write!(f, "the tokenizer: {error}"),
            Self::Model(error) => write!(f, "the model: {error}"),
            Self::TooLarge(bytes) => write!(
                f,
                "its vocabulary and tables need {} MiB, more than the engine may use",
                bytes.div_ceil(1 << 20)
            ),
            Self::NoContext(positions) => write!(
                f,
                "its KV cache fits only {positions} positions in the engine's memory"
            ),
        }
    }
}

/// What the engine knows about the model it loaded, for `health` and `:model`.
#[derive(Clone, Debug, PartialEq)]
pub struct About {
    /// `general.name`, made printable and at most 64 characters.
    pub name: Option<String>,
    /// `general.license`, the same way.
    pub license: Option<String>,
    pub config: Config,
    /// Positions of context the KV cache holds.
    pub context: usize,
    pub kv_bytes: usize,
    pub tier: Tier,
}

/// One step of an answer, given to the callback once it is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// A prompt token went through the model.
    Prompt,
    /// The model made this token of the answer.
    Token(u32),
}

/// How an answer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// The model ended it with `<|im_end|>` or `<|endoftext|>`.
    EndToken,
    MaxTokens,
    ContextFull,
    /// The callback said stop.
    Stopped,
}

/// An answer: the bytes its tokens stand for and how it went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub bytes: Vec<u8>,
    pub prompt_tokens: usize,
    /// Tokens made, the end token not counted.
    pub tokens: usize,
    pub ending: Ending,
}

impl Reply {
    /// The answer as text; bytes that are not UTF-8 become U+FFFD.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    /// The model did not end the answer itself.
    pub fn cut_off(&self) -> bool {
        self.ending != Ending::EndToken
    }
}

pub struct Engine<'a> {
    model: Model<'a>,
    kv: KvCache,
    tok: Tokenizer,
    chat: Chat,
    logits: Vec<f32>,
    about: About,
}

impl<'a> Engine<'a> {
    /// Checks the file, builds its tokenizer and sizes the KV cache to at most
    /// `context_cap` positions within `budget`.
    pub fn load(
        bytes: &'a [u8],
        tier: Tier,
        context_cap: usize,
        budget: Budget,
    ) -> Result<Self, LoadError> {
        let file = Gguf::parse(bytes).map_err(LoadError::File)?;
        let config = Config::from_gguf(file.header()).map_err(LoadError::Model)?;
        crate::check(file.header(), &config).map_err(LoadError::Model)?;
        let cap = context_cap.min(config.context_length);
        let other = other_bytes(&file, &config, cap);
        if other > budget.other_bytes {
            return Err(LoadError::TooLarge(other));
        }
        let per_position = config.kv_bytes(1).ok_or(LoadError::TooLarge(usize::MAX))?;
        let context = cap.min(budget.kv_bytes / per_position);
        if context < MIN_CONTEXT {
            return Err(LoadError::NoContext(context));
        }
        let tok = Tokenizer::from_gguf(file.header()).map_err(LoadError::Tokenizer)?;
        let chat = Chat::new(&tok).map_err(LoadError::Tokenizer)?;
        let model = Model::new(&file, tier, context).map_err(LoadError::Model)?;
        let kv = model.kv_cache().map_err(LoadError::Model)?;
        let mut logits = Vec::new();
        logits
            .try_reserve_exact(config.vocab)
            .map_err(|_| LoadError::Model(Error::OutOfMemory))?;
        logits.resize(config.vocab, 0.0);
        let text = |key: &str| {
            file.get(key)
                .and_then(|value| value.as_str())
                .map(shown)
                .filter(|text| !text.is_empty())
        };
        let about = About {
            name: text("general.name"),
            license: text("general.license"),
            config,
            context,
            kv_bytes: kv.bytes(),
            tier,
        };
        Ok(Self {
            model,
            kv,
            tok,
            chat,
            logits,
            about,
        })
    }

    pub fn about(&self) -> &About {
        &self.about
    }

    /// Answers `user` after the optional `system` prompt, starting from an empty
    /// context, with at most `max_new` tokens. `on_step` runs after each prompt
    /// token and each token of the answer; when it says stop, the answer ends
    /// there. A prompt longer than the context is [`Error::ContextFull`].
    pub fn answer(
        &mut self,
        system: Option<&str>,
        user: &str,
        max_new: usize,
        mut on_step: impl FnMut(Step) -> Next,
    ) -> Result<Reply, Error> {
        let prompt = self.chat.prompt(&self.tok, system, user);
        if prompt.is_empty() {
            return Err(Error::EmptyPrompt);
        }
        if prompt.len() > self.kv.capacity() {
            return Err(Error::ContextFull);
        }
        self.kv.clear();
        let mut reply = Reply {
            bytes: Vec::new(),
            prompt_tokens: prompt.len(),
            tokens: 0,
            ending: Ending::MaxTokens,
        };
        for (i, &token) in prompt.iter().enumerate() {
            let last = i + 1 == prompt.len();
            let logits = last.then_some(self.logits.as_mut_slice());
            self.model.forward(&mut self.kv, token, logits)?;
            if on_step(Step::Prompt) == Next::Stop {
                reply.ending = Ending::Stopped;
                return Ok(reply);
            }
        }
        for made in 1..=max_new {
            let token = argmax(&self.logits);
            if self.chat.is_stop(token) {
                reply.ending = Ending::EndToken;
                return Ok(reply);
            }
            self.tok.decode(&[token], &mut reply.bytes);
            reply.tokens += 1;
            if on_step(Step::Token(token)) == Next::Stop {
                reply.ending = Ending::Stopped;
                return Ok(reply);
            }
            if made == max_new {
                break;
            }
            if self.kv.len() >= self.kv.capacity() {
                reply.ending = Ending::ContextFull;
                return Ok(reply);
            }
            self.model
                .forward(&mut self.kv, token, Some(&mut self.logits))?;
        }
        Ok(reply)
    }

    /// The prompt's length in tokens, for a caller sizing `max_new`.
    pub fn prompt_tokens(&self, system: Option<&str>, user: &str) -> usize {
        self.chat.prompt(&self.tok, system, user).len()
    }
}

/// An upper bound on the bytes the engine allocates for `file` besides the KV
/// cache, read from the header alone: the tokenizer while it is built (each
/// growing vector counted three times over, for a doubling copy and its old
/// buffer), the header's metadata map, one f32 logit per token, the RoPE table and
/// the scratch vectors.
fn other_bytes(file: &Gguf<'_>, config: &Config, positions: usize) -> usize {
    let strings = |key: &str| -> (usize, usize) {
        file.get(key)
            .and_then(|value| value.as_array())
            .and_then(|array| array.strs())
            .map_or((0, 0), |strs| {
                strs.fold((0usize, 0usize), |(count, bytes), s| {
                    (count + 1, bytes.saturating_add(s.len()))
                })
            })
    };
    let (tokens, token_bytes) = strings("tokenizer.ggml.tokens");
    let (merges, _) = strings("tokenizer.ggml.merges");
    let f32s = |n: usize| n.saturating_mul(4);
    let tokenizer = token_bytes
        .saturating_mul(3)
        .saturating_add(tokens.saturating_mul(4 + 1 + 3 * 4))
        .saturating_add(merges.saturating_mul(3 * 16));
    let scratch = f32s(
        config
            .n_embd
            .saturating_mul(5)
            .saturating_add(config.q_dim().saturating_mul(2))
            .saturating_add(config.kv_dim().saturating_mul(2))
            .saturating_add(config.n_ff.saturating_mul(4))
            .saturating_add(positions),
    );
    let norms = f32s(
        config
            .n_layer
            .saturating_mul(config.n_embd.saturating_mul(2) + config.head_dim * 2),
    );
    let rope = f32s(positions.saturating_mul(config.head_dim));
    // The parsed header's metadata map lives as long as the load does.
    let metadata = file.header().metadata().count().saturating_mul(256);
    tokenizer
        .saturating_add(metadata)
        .saturating_add(f32s(config.vocab))
        .saturating_add(scratch)
        .saturating_add(norms)
        .saturating_add(rope)
        .saturating_add(SLACK_BYTES)
}

/// Metadata text as a health line may show it: printable, one line, at most 64
/// characters.
fn shown(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .into()
}
