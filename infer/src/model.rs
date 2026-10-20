//! The qwen3 forward pass over blocks of tokens: RMSNorm, projections in each
//! matrix's own format (Q8_0, Q4_K or Q6_K), per-head QK-norm, NeoX RoPE,
//! grouped-query attention over the f16 KV cache ([`crate::attend`]), a SwiGLU
//! feed-forward block, and the output projection (tied to the embeddings in
//! Qwen3-0.6B and 4B). Norms and softmax sum in f64 and the RoPE angles are built
//! as llama.cpp builds them, so greedy answers follow llama.cpp's as closely as
//! an f16 cache allows.
//!
//! A prompt runs in blocks of up to [`PREFILL_TOKENS`] tokens: each matrix row is
//! read once per block and multiplied by every token's vector with the same
//! kernel a single token uses, and each token attends over exactly the positions
//! before it, so a block gives the bits of its tokens run one at a time.
//! Matrices that share an input run in one [`matvec_parallel`] call on the
//! model's [`Parallel`] workers, with the same bits as one thread.

use crate::acts::Acts;
use crate::attend::{attend, scratch_len};
use crate::weights::Matrix;
use crate::{Config, Error, KvCache, Rope, Weights};
use alloc::vec;
use alloc::vec::Vec;
use gguf::Gguf;
use quant::{Parallel, Serial, Tier, matvec_parallel};

/// Tokens a prompt block runs at once.
pub const PREFILL_TOKENS: usize = 32;

/// Buffers for one block, allocated once; per-token vectors lie one after
/// another.
struct Scratch {
    x: Vec<f32>,
    xn: Vec<f32>,
    q: Vec<f32>,
    k: Vec<f32>,
    v: Vec<f32>,
    att: Vec<f32>,
    gate: Vec<f32>,
    up: Vec<f32>,
    out: Vec<f32>,
    /// Products of a block, row by row, before they are laid out by token.
    rows: Vec<f32>,
    /// Each attention head's output and scores.
    heads: Vec<f32>,
    q_embd: Acts,
    q_att: Acts,
    q_ff: Acts,
}

pub struct Model<'a> {
    config: Config,
    weights: Weights<'a>,
    tier: Tier,
    par: &'a dyn Parallel,
    rope: Rope,
    context: usize,
    s: Scratch,
}

/// Floats in [`Scratch::rows`]: the most rows a call multiplies at once.
fn widest(c: &Config) -> usize {
    (c.q_dim() + 2 * c.kv_dim()).max(2 * c.n_ff).max(c.n_embd)
}

impl<'a> Model<'a> {
    /// Checks the file and sets it up for up to `context` tokens of context.
    pub fn new(file: &Gguf<'a>, tier: Tier, context: usize) -> Result<Self, Error> {
        let config = Config::from_gguf(file.header())?;
        if context == 0 || context > config.context_length {
            return Err(Error::Context);
        }
        let weights = Weights::bind(file, &config)?;
        let rope = Rope::new(&config, context)?;
        let (e, q, kv, ff) = (config.n_embd, config.q_dim(), config.kv_dim(), config.n_ff);
        let layers = &weights.layers;
        let embd_readers = layers
            .iter()
            .flat_map(|l| [&l.wq, &l.wk, &l.wv, &l.w_gate, &l.w_up])
            .chain([&weights.output]);
        let n = PREFILL_TOKENS;
        let s = Scratch {
            x: vec![0.0; n * e],
            xn: vec![0.0; n * e],
            q: vec![0.0; n * q],
            k: vec![0.0; n * kv],
            v: vec![0.0; n * kv],
            att: vec![0.0; n * q],
            gate: vec![0.0; n * ff],
            up: vec![0.0; n * ff],
            out: vec![0.0; n * e],
            rows: vec![0.0; n * widest(&config)],
            heads: vec![0.0; scratch_len(&config, context)],
            q_embd: Acts::new(e, n, embd_readers),
            q_att: Acts::new(q, n, layers.iter().map(|l| &l.wo)),
            q_ff: Acts::new(ff, n, layers.iter().map(|l| &l.w_down)),
        };
        Ok(Self {
            config,
            weights,
            tier,
            par: &Serial,
            rope,
            context,
            s,
        })
    }

    /// A KV cache for this model's whole context cap.
    pub fn kv_cache(&self) -> Result<KvCache, Error> {
        KvCache::new(&self.config, self.context)
    }

    /// The shape the model was checked and sized for.
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Runs the matrix products and attention heads on `par`'s workers from now
    /// on; [`Serial`] until this is called. Any pool gives the same bits.
    pub fn set_parallel(&mut self, par: &'a dyn Parallel) {
        self.par = par;
    }

    pub fn context(&self) -> usize {
        self.context
    }

    /// Runs `token` at the next position of `kv`: [`Model::forward_batch`] of one.
    pub fn forward(
        &mut self,
        kv: &mut KvCache,
        token: u32,
        logits: Option<&mut [f32]>,
    ) -> Result<(), Error> {
        self.forward_batch(kv, &[token], logits)
    }

    /// Runs `tokens` at the next positions of `kv`, in blocks of
    /// [`PREFILL_TOKENS`], filling `logits` (one per vocabulary entry) for the
    /// last token when it is given; earlier tokens skip the output projection,
    /// the largest matrix. The bits are those of running the tokens one at a
    /// time. Every check comes before the cache changes, so an error leaves it as
    /// it was.
    pub fn forward_batch(
        &mut self,
        kv: &mut KvCache,
        tokens: &[u32],
        logits: Option<&mut [f32]>,
    ) -> Result<(), Error> {
        let c = self.config;
        if tokens.is_empty() {
            return Err(Error::EmptyPrompt);
        }
        if let Some(&bad) = tokens.iter().find(|&&t| t as usize >= c.vocab) {
            return Err(Error::Token(bad));
        }
        if logits.as_deref().is_some_and(|l| l.len() != c.vocab) {
            return Err(Error::Logits);
        }
        if !self.fits(kv) {
            return Err(Error::CacheShape);
        }
        if tokens.len() > kv.capacity() - kv.len() {
            return Err(Error::ContextFull);
        }
        let mut last = 0;
        for block in tokens.chunks(PREFILL_TOKENS) {
            self.block(kv, block);
            last = block.len() - 1;
        }
        if let Some(logits) = logits {
            let e = c.n_embd;
            let (w, s) = (&self.weights, &mut self.s);
            rms_norm(
                &s.x[last * e..][..e],
                &w.output_norm,
                c.rms_eps,
                &mut s.xn[..e],
            );
            s.q_embd.quantize(0, &s.xn[..e]);
            let jobs = &mut [(&w.output, logits)];
            run(self.par, self.tier, &s.q_embd, 1, &mut s.rows, jobs);
        }
        Ok(())
    }

    /// Whether `kv` was made for this model: its shape, and no more positions
    /// than the RoPE table covers.
    pub fn fits(&self, kv: &KvCache) -> bool {
        kv.fits(&self.config) && kv.capacity() <= self.context
    }

    /// Runs one block of at most [`PREFILL_TOKENS`] checked tokens.
    fn block(&mut self, kv: &mut KvCache, tokens: &[u32]) {
        let e = self.config.n_embd;
        let embd = self.weights.token_embd;
        for (&token, x) in tokens.iter().zip(self.s.x.chunks_exact_mut(e)) {
            embd.dequantize_row(token as usize, x);
        }
        for layer in 0..self.config.n_layer {
            self.attention(kv, layer, tokens.len());
            self.feed_forward(layer, tokens.len());
        }
        kv.advance(tokens.len());
    }

    fn attention(&mut self, kv: &mut KvCache, layer: usize, n: usize) {
        let Self {
            config: c,
            weights,
            tier,
            par,
            rope,
            context,
            s,
        } = self;
        let (e, hd, qd, kvd) = (c.n_embd, c.head_dim, c.q_dim(), c.kv_dim());
        let w = &weights.layers[layer];
        let pos = kv.len();
        for t in 0..n {
            rms_norm(
                &s.x[t * e..][..e],
                &w.attn_norm,
                c.rms_eps,
                &mut s.xn[t * e..][..e],
            );
            s.q_embd.quantize(t, &s.xn[t * e..][..e]);
        }
        let qkv = &mut [
            (&w.wq, &mut s.q[..n * qd]),
            (&w.wk, &mut s.k[..n * kvd]),
            (&w.wv, &mut s.v[..n * kvd]),
        ];
        run(*par, *tier, &s.q_embd, n, &mut s.rows, qkv);
        for t in 0..n {
            for head in s.q[t * qd..][..qd].chunks_exact_mut(hd) {
                rms_norm_in_place(head, &w.q_norm, c.rms_eps);
                rope.apply(head, pos + t);
            }
            let k = &mut s.k[t * kvd..][..kvd];
            for head in k.chunks_exact_mut(hd) {
                rms_norm_in_place(head, &w.k_norm, c.rms_eps);
                rope.apply(head, pos + t);
            }
            let (k_slot, v_slot) = kv.slot_mut(layer, pos + t);
            for (slot, &k) in k_slot.iter_mut().zip(&*k) {
                *slot = quant::f16::from_f32(k);
            }
            for (slot, &v) in v_slot.iter_mut().zip(&s.v[t * kvd..][..kvd]) {
                *slot = quant::f16::from_f32(v);
            }
        }
        let cache = kv.layer(layer, pos + n - 1);
        for t in 0..n {
            let (q, att) = (&s.q[t * qd..][..qd], &mut s.att[t * qd..][..qd]);
            let heads = (&mut s.heads[..], *context);
            attend(*par, *tier, c, q, cache, pos + t + 1, heads, att);
            s.q_att.quantize(t, att);
        }
        run(
            *par,
            *tier,
            &s.q_att,
            n,
            &mut s.rows,
            &mut [(&w.wo, &mut s.out[..n * e])],
        );
        for (x, o) in s.x[..n * e].iter_mut().zip(&s.out) {
            *x += o;
        }
    }

    fn feed_forward(&mut self, layer: usize, n: usize) {
        let (c, e, ff) = (self.config, self.config.n_embd, self.config.n_ff);
        let w = &self.weights.layers[layer];
        let s = &mut self.s;
        for t in 0..n {
            rms_norm(
                &s.x[t * e..][..e],
                &w.ffn_norm,
                c.rms_eps,
                &mut s.xn[t * e..][..e],
            );
            s.q_embd.quantize(t, &s.xn[t * e..][..e]);
        }
        let gate_up = &mut [
            (&w.w_gate, &mut s.gate[..n * ff]),
            (&w.w_up, &mut s.up[..n * ff]),
        ];
        run(self.par, self.tier, &s.q_embd, n, &mut s.rows, gate_up);
        for (g, u) in s.gate[..n * ff].iter_mut().zip(&s.up) {
            *g = *g / (1.0 + libm::expf(-*g)) * u;
        }
        for t in 0..n {
            s.q_ff.quantize(t, &s.gate[t * ff..][..ff]);
        }
        let down = &mut [(&w.w_down, &mut s.out[..n * e])];
        run(self.par, self.tier, &s.q_ff, n, &mut s.rows, down);
        for (x, o) in s.x[..n * e].iter_mut().zip(&s.out) {
            *x += o;
        }
    }
}

/// An upper bound on the bytes [`Model::new`] allocates for `context` positions
/// besides the norms and the RoPE table: one block's scratch vectors and
/// activations in both formats, and the attention heads' scratch.
pub(crate) fn scratch_bytes(c: &Config, context: usize) -> usize {
    let (e, q, kv, ff) = (c.n_embd, c.q_dim(), c.kv_dim(), c.n_ff);
    let per_token = e
        .saturating_mul(3)
        .saturating_add(q.saturating_mul(2))
        .saturating_add(kv.saturating_mul(2))
        .saturating_add(ff.saturating_mul(2))
        .saturating_add(widest(c));
    let f32s = per_token
        .saturating_mul(PREFILL_TOKENS)
        .saturating_add(scratch_len(c, context));
    let acts = Acts::bytes(e, PREFILL_TOKENS)
        .saturating_add(Acts::bytes(q, PREFILL_TOKENS))
        .saturating_add(Acts::bytes(ff, PREFILL_TOKENS));
    f32s.saturating_mul(4).saturating_add(acts)
}

/// Matrices that share the input `x` times its first `n` tokens, in one call on
/// `par`. Each output holds `rows * n` values, token by token; with more than one
/// token the products go row by row through `rows` first.
fn run<const N: usize>(
    par: &dyn Parallel,
    tier: Tier,
    x: &Acts,
    n: usize,
    rows: &mut [f32],
    jobs: &mut [(&Matrix<'_>, &mut [f32]); N],
) {
    if n == 1 {
        let mut items = jobs.each_mut().map(|(m, y)| (x.times(m, 1), &mut **y));
        matvec_parallel(par, tier, &mut items);
        return;
    }
    let mut rest = rows;
    let mut parts: [&mut [f32]; N] = core::array::from_fn(|i| {
        let (part, after) = core::mem::take(&mut rest).split_at_mut(jobs[i].0.rows * n);
        rest = after;
        part
    });
    let mut pairs = jobs.iter().zip(parts.iter_mut());
    let mut items: [_; N] = core::array::from_fn(|_| {
        let ((m, _), part) = pairs.next().expect("one part per job");
        (x.times(m, n), &mut **part)
    });
    matvec_parallel(par, tier, &mut items);
    for ((m, y), part) in jobs.iter_mut().zip(&parts) {
        for (r, row) in part.chunks_exact(n).enumerate() {
            for (t, &v) in row.iter().enumerate() {
                y[t * m.rows + r] = v;
            }
        }
    }
}

/// `x / rms(x) * weight`; the sum of squares runs in f64, as in llama.cpp.
fn rms_norm(x: &[f32], weight: &[f32], eps: f32, out: &mut [f32]) {
    let scale = rms_scale(x, eps);
    for ((o, &v), &w) in out.iter_mut().zip(x).zip(weight) {
        *o = v * scale * w;
    }
}

fn rms_norm_in_place(x: &mut [f32], weight: &[f32], eps: f32) {
    let scale = rms_scale(x, eps);
    for (v, &w) in x.iter_mut().zip(weight) {
        *v = *v * scale * w;
    }
}

fn rms_scale(x: &[f32], eps: f32) -> f32 {
    let sum: f64 = x.iter().map(|&v| f64::from(v * v)).sum();
    let mean = (sum / x.len() as f64) as f32;
    1.0 / libm::sqrtf(mean + eps)
}
