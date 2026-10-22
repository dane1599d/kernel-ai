//! One answer from a model file, run the same way by the llm service inside Kernel
//! AI and by xtask on the host: the ChatML prompt of a conversation with thinking
//! off, then decoding (greedy, or seeded sampling with a [`Sampler`], optionally
//! constrained to a JSON [`Shape`]) until an end token, the closed object, a
//! field's limit, the token limit or the end of the context. A callback after
//! every step, prompt tokens included, is where the caller sleeps, keeps a
//! deadline or stops.
//!
//! The KV cache outlives a request: the engine remembers the token at every
//! position and runs a new prompt only from where it parts from them, as
//! llama.cpp's `cache_prompt` does, so a system prompt every request starts with
//! is computed once. Keys and values at a position depend only on the tokens up
//! to it, so the answer is the one a fresh engine gives, bit for bit.
//!
//! [`Engine::load`] treats the file as hostile: before it allocates anything sized
//! by the file it estimates what the tokenizer, the logits and the RoPE table will
//! take and refuses a file that would not fit [`Budget::other_bytes`], so an
//! oversized vocabulary fails with an error instead of exhausting the heap. The KV
//! cache then gets as many positions as [`Budget::kv_bytes`] holds, at most the
//! context cap and the model's trained context.

use crate::json::{Grammar, Overflow, Shape, Stop, ascii_tokens, spells_ascii};
use crate::{Config, Error, KvCache, Model, Next, Sampler};
use alloc::string::String;
use alloc::vec::Vec;
use bpe::Tokenizer;
use bpe::chat::Chat;
pub use bpe::chat::{Role, Turn};
use gguf::Gguf;
use quant::{Parallel, Tier};

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
    /// logits, the RoPE table, the scratch vectors and the token at each cached
    /// position.
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
    /// Workers the matrix products and attention heads run on.
    pub workers: usize,
}

/// One step of an answer, given to the callback once it is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Once, first: this many prompt tokens were in the cache already.
    Cached(usize),
    /// A prompt token went through the model; a block of them (up to
    /// [`crate::PREFILL_TOKENS`]) runs at once, then one step comes per token.
    Prompt,
    /// The model made this token of the answer.
    Token(u32),
}

/// How an answer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// The model ended it with `<|im_end|>` or `<|endoftext|>`, or closed the
    /// object its JSON shape asks for.
    EndToken,
    /// The token limit came, or under a shape, the tokens left were too few to
    /// close the object, counting one token per byte it still needs.
    MaxTokens,
    /// Likewise for the positions left in the context.
    ContextFull,
    /// The callback said stop.
    Stopped,
    /// Under a shape, the model picked a token past a field's limit; the answer
    /// ends before it.
    Overflow(Overflow),
}

/// An answer: the bytes its tokens stand for and how it went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub bytes: Vec<u8>,
    pub prompt_tokens: usize,
    /// Prompt tokens whose keys and values came from the cache, not the model.
    pub cached: usize,
    /// Tokens made, the end token not counted.
    pub tokens: usize,
    /// Of `tokens`, the ones jump-forward fed as a block (see [`Shape`]).
    pub forced: usize,
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
    /// The token at each position of `kv`, so a prompt that starts like the last
    /// one runs only from where they part.
    cached: Vec<u32>,
    /// The tokens a JSON shape may ever allow: those of printable ASCII alone.
    ascii: Vec<u32>,
    /// The ones it allows at a step, room for all of them.
    candidates: Vec<u32>,
    /// Every printable ASCII byte has a token of its own, as a shape needs.
    spells_ascii: bool,
    /// Tokens jump-forward fed in the last reply.
    forced: usize,
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
        let mut cached = Vec::new();
        cached
            .try_reserve_exact(kv.capacity())
            .map_err(|_| LoadError::Model(Error::OutOfMemory))?;
        let mut logits = Vec::new();
        logits
            .try_reserve_exact(config.vocab)
            .map_err(|_| LoadError::Model(Error::OutOfMemory))?;
        logits.resize(config.vocab, 0.0);
        let ascii = ascii_tokens(&tok).map_err(LoadError::Model)?;
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(ascii.len())
            .map_err(|_| LoadError::Model(Error::OutOfMemory))?;
        let spells_ascii = spells_ascii(&tok, &ascii);
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
            workers: 1,
        };
        Ok(Self {
            model,
            kv,
            tok,
            chat,
            logits,
            cached,
            ascii,
            candidates,
            spells_ascii,
            forced: 0,
            about,
        })
    }

    /// Runs the model on `par`'s workers from now on (one thread until this is
    /// called), with the same bits.
    pub fn set_parallel(&mut self, par: &'a dyn Parallel) {
        self.model.set_parallel(par);
        self.about.workers = par.workers();
    }

    pub fn about(&self) -> &About {
        &self.about
    }

    /// Answers `user` after the optional `system` prompt, greedily: [`Engine::reply`]
    /// to that one question.
    pub fn answer(
        &mut self,
        system: Option<&str>,
        user: &str,
        max_new: usize,
        on_step: impl FnMut(Step) -> Next,
    ) -> Result<Reply, Error> {
        let (both, one);
        let user = Turn::new(Role::User, user);
        let turns: &[Turn<'_>] = match system {
            Some(system) => {
                both = [Turn::new(Role::System, system), user];
                &both
            }
            None => {
                one = [user];
                &one
            }
        };
        self.reply(turns, max_new, &mut Sampler::greedy(), None, on_step)
    }

    /// The assistant's next turn after `turns`, with at most `max_new` tokens
    /// picked by `sampler`; the prompt runs from where it parts from the cache.
    /// Under a `shape` (see [`crate::json`]) every token keeps the answer a prefix
    /// of that object: it ends when the object closes, at an [`Ending::Overflow`],
    /// or cut off when the tokens or positions left are too few to close it,
    /// counting one per byte (on the safe side); [`Error::NoRoom`] comes before
    /// anything runs when they are too few from the start. `on_step` gets
    /// [`Step::Cached`] once, then runs after each prompt token the cache did not
    /// hold and each token of the answer; when it says stop, the answer ends
    /// there. A prompt longer than the context is [`Error::ContextFull`].
    pub fn reply(
        &mut self,
        turns: &[Turn<'_>],
        max_new: usize,
        sampler: &mut Sampler,
        shape: Option<&Shape<'_>>,
        mut on_step: impl FnMut(Step) -> Next,
    ) -> Result<Reply, Error> {
        let grammar = match shape {
            Some(_) if !self.spells_ascii => {
                return Err(Error::Shape("needs a token for every printable ASCII byte"));
            }
            Some(shape) => Some(Grammar::new(shape)?),
            None => None,
        };
        let prompt = self.chat.conversation(&self.tok, turns);
        if prompt.is_empty() {
            return Err(Error::EmptyPrompt);
        }
        if prompt.len() > self.kv.capacity() {
            return Err(Error::ContextFull);
        }
        self.forced = 0;
        let mut state = grammar.as_ref().map(Grammar::start);
        if let (Some(grammar), Some(start)) = (&grammar, &state) {
            // Room to close the object: one token per byte, and one position per
            // token but the last.
            if grammar.need(start) > max_new.min(self.kv.capacity() - prompt.len() + 1) {
                return Err(Error::NoRoom);
            }
        }
        let (cached, stopped) = self.feed(&prompt, &mut on_step)?;
        let mut reply = Reply {
            bytes: Vec::new(),
            prompt_tokens: prompt.len(),
            cached,
            tokens: 0,
            forced: 0,
            ending: Ending::MaxTokens,
        };
        if stopped {
            reply.ending = Ending::Stopped;
            return Ok(reply);
        }
        let jump = shape.is_some_and(|shape| shape.jump_forward);
        // The tokens the next model step runs: the one just picked, then any the
        // shape forces after it.
        let mut block = Vec::new();
        // The object's opening literal is forced too, right after the prompt.
        if jump && let (Some(grammar), Some(state)) = (&grammar, &mut state) {
            if let Some(ending) = self.jump(grammar, state, &mut reply, &mut block, &mut on_step) {
                reply.ending = ending;
                return Ok(reply);
            }
            if !block.is_empty() {
                self.model
                    .forward_batch(&mut self.kv, &block, Some(&mut self.logits))?;
                self.cached.extend_from_slice(&block);
            }
        }
        let mut made = reply.tokens;
        while made < max_new {
            let token = match (&grammar, &mut state) {
                (Some(grammar), Some(state)) => match self.shaped(grammar, state, sampler)? {
                    Ok(token) => token,
                    Err(overflow) => {
                        reply.ending = Ending::Overflow(overflow);
                        return Ok(reply);
                    }
                },
                _ => {
                    let token = sampler.pick(&self.logits);
                    if self.chat.is_stop(token) {
                        reply.ending = Ending::EndToken;
                        return Ok(reply);
                    }
                    token
                }
            };
            made += 1;
            self.tok.decode(&[token], &mut reply.bytes);
            reply.tokens += 1;
            let next = on_step(Step::Token(token));
            if let (Some(grammar), Some(state)) = (&grammar, &state) {
                if state.is_done() {
                    reply.ending = Ending::EndToken;
                    return Ok(reply);
                }
                // Tokens still to pick, and positions for all of them but the last.
                let (tokens, positions) = (max_new - made, self.kv.capacity() - self.kv.len());
                if grammar.need(state) > tokens.min(positions) {
                    reply.ending = match tokens <= positions {
                        true => Ending::MaxTokens,
                        false => Ending::ContextFull,
                    };
                    return Ok(reply);
                }
            }
            if next == Next::Stop {
                reply.ending = Ending::Stopped;
                return Ok(reply);
            }
            if made == max_new {
                break;
            }
            block.clear();
            block.push(token);
            // `need` held above, so the forced tokens fit the tokens and positions
            // left: each is at least one of the bytes it counted.
            if jump && let (Some(grammar), Some(state)) = (&grammar, &mut state) {
                if let Some(ending) =
                    self.jump(grammar, state, &mut reply, &mut block, &mut on_step)
                {
                    reply.ending = ending;
                    return Ok(reply);
                }
                made = reply.tokens;
            }
            if self.kv.len() + block.len() > self.kv.capacity() {
                reply.ending = Ending::ContextFull;
                return Ok(reply);
            }
            match block[..] {
                [one] => self
                    .model
                    .forward(&mut self.kv, one, Some(&mut self.logits))?,
                _ => self
                    .model
                    .forward_batch(&mut self.kv, &block, Some(&mut self.logits))?,
            }
            self.cached.extend_from_slice(&block);
        }
        Ok(reply)
    }

    /// Jump-forward: the bytes `grammar` forces after `state`, as the tokenizer
    /// splits them, are taken as the answer's next tokens without asking the
    /// model, appended to `block` for one model step, and `state` moves past
    /// them. Nothing happens if they are none, or if a token is not one the shape
    /// allows (step decoding then makes them). `Some` when the answer ends in
    /// them: the object closed there, or `on_step` said stop.
    fn jump(
        &mut self,
        grammar: &Grammar<'_>,
        state: &mut crate::json::State,
        reply: &mut Reply,
        block: &mut Vec<u32>,
        on_step: &mut impl FnMut(Step) -> Next,
    ) -> Option<Ending> {
        let mut bytes = Vec::new();
        grammar.forced(state, &mut bytes);
        let text = core::str::from_utf8(&bytes)
            .ok()
            .filter(|text| !text.is_empty())?;
        let start = block.len();
        self.tok.encode(text, block);
        let mut next = *state;
        for &id in &block[start..] {
            let allowed = self.ascii.binary_search(&id).is_ok();
            match grammar.advance(next, self.tok.token_bytes(id)) {
                Ok(after) if allowed => next = after,
                _ => {
                    block.truncate(start);
                    return None;
                }
            }
        }
        *state = next;
        for &id in &block[start..] {
            self.tok.decode(&[id], &mut reply.bytes);
            reply.tokens += 1;
            reply.forced += 1;
            self.forced += 1;
            if on_step(Step::Token(id)) == Next::Stop {
                return Some(Ending::Stopped);
            }
        }
        state.is_done().then_some(Ending::EndToken)
    }

    /// Runs `prompt` through the model after the longest start it shares with the
    /// tokens the cache holds, which stay; the last token runs whatever is cached,
    /// for its logits. The rest runs in blocks of [`crate::PREFILL_TOKENS`], with
    /// a [`Step::Prompt`] per token after each block. Returns how many tokens came
    /// from the cache, and whether `on_step` said stop. Every check in
    /// `forward_batch` comes before the cache changes, so the tokens behind it
    /// stay exactly those it holds.
    fn feed(
        &mut self,
        prompt: &[u32],
        on_step: &mut impl FnMut(Step) -> Next,
    ) -> Result<(usize, bool), Error> {
        let shared = self.cached.iter().zip(prompt).take_while(|(a, b)| a == b);
        let cached = shared.count().min(prompt.len() - 1);
        self.kv.truncate(cached);
        self.cached.truncate(cached);
        if on_step(Step::Cached(cached)) == Next::Stop {
            return Ok((cached, true));
        }
        let rest = &prompt[cached..];
        let blocks = rest.chunks(crate::PREFILL_TOKENS).count();
        for (b, block) in rest.chunks(crate::PREFILL_TOKENS).enumerate() {
            let logits = (b + 1 == blocks).then_some(self.logits.as_mut_slice());
            self.model.forward_batch(&mut self.kv, block, logits)?;
            self.cached.extend_from_slice(block);
            for _ in block {
                if on_step(Step::Prompt) == Next::Stop {
                    return Ok((cached, true));
                }
            }
        }
        Ok((cached, false))
    }

    /// The model's pick among the tokens `grammar` allows after `state`, which
    /// moves on past it, or the limit the pick went past.
    fn shaped(
        &mut self,
        grammar: &Grammar<'_>,
        state: &mut crate::json::State,
        sampler: &mut Sampler,
    ) -> Result<Result<u32, Overflow>, Error> {
        grammar.allowed(state, &self.tok, &self.ascii, &mut self.candidates);
        let token = sampler
            .pick_from(&self.logits, &self.candidates)
            .ok_or(Error::Shape("allowed no token"))?;
        match grammar.advance(*state, self.tok.token_bytes(token)) {
            Ok(next) => {
                *state = next;
                Ok(Ok(token))
            }
            Err(Stop::Over(overflow)) => Ok(Err(overflow)),
            Err(Stop::Masked) => Err(Error::Shape("allowed a token it cannot take")),
        }
    }

    /// The tokens behind the cached positions, and the cache, for tests.
    #[cfg(test)]
    pub(crate) fn cache(&self) -> (&[u32], &KvCache) {
        (&self.cached, &self.kv)
    }

    /// Tokens jump-forward fed in the last reply, of those it made.
    pub fn forced(&self) -> usize {
        self.forced
    }

    /// The prompt's length in tokens, for a caller sizing `max_new`.
    pub fn prompt_tokens(&self, system: Option<&str>, user: &str) -> usize {
        self.chat.prompt(&self.tok, system, user).len()
    }
}

/// An upper bound on the bytes the engine allocates for `file` besides the KV
/// cache, read from the header alone: the tokenizer while it is built (each
/// growing vector counted three times over, for a doubling copy and its old
/// buffer), the header's metadata map, one f32 logit and two token ids (for JSON
/// shapes) per token, the RoPE table, the scratch vectors and one token id per
/// position.
pub(crate) fn other_bytes(file: &Gguf<'_>, config: &Config, positions: usize) -> usize {
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
    // The model's buffers, and the token behind each cached position.
    let scratch = crate::model::scratch_bytes(config, positions).saturating_add(f32s(positions));
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
        // The logits, and the ids a JSON shape may allow and allows.
        .saturating_add(f32s(config.vocab).saturating_mul(3))
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
