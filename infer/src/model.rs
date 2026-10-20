//! The qwen3 forward pass for one token: RMSNorm, projections in each matrix's
//! own format (Q8_0, Q4_K or Q6_K), per-head QK-norm, NeoX RoPE, grouped-query
//! attention over the f32 KV cache, a SwiGLU feed-forward block, and the output
//! projection (tied to the embeddings in Qwen3-0.6B and 4B). Norms and softmax
//! sum in f64 and the RoPE angles are built as llama.cpp builds them, so greedy
//! answers follow llama.cpp's as closely as an f32 cache allows. Matrices that
//! share an input run in one [`matvec_parallel`] call on the model's
//! [`Parallel`] workers, with the same bits as one thread.

use crate::acts::Acts;
use crate::weights::Matrix;
use crate::{Config, Error, KvCache, Rope, Weights};
use alloc::vec;
use alloc::vec::Vec;
use gguf::Gguf;
use quant::{Parallel, Serial, Tier, matvec_parallel};

/// Buffers for one token, allocated once.
struct Scratch {
    x: Vec<f32>,
    xn: Vec<f32>,
    q: Vec<f32>,
    k: Vec<f32>,
    v: Vec<f32>,
    att: Vec<f32>,
    scores: Vec<f32>,
    gate: Vec<f32>,
    up: Vec<f32>,
    out: Vec<f32>,
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
        let s = Scratch {
            x: vec![0.0; e],
            xn: vec![0.0; e],
            q: vec![0.0; q],
            k: vec![0.0; kv],
            v: vec![0.0; kv],
            att: vec![0.0; q],
            scores: vec![0.0; context],
            gate: vec![0.0; ff],
            up: vec![0.0; ff],
            out: vec![0.0; e],
            q_embd: Acts::new(e, 1, embd_readers),
            q_att: Acts::new(q, 1, layers.iter().map(|l| &l.wo)),
            q_ff: Acts::new(ff, 1, layers.iter().map(|l| &l.w_down)),
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

    /// Runs the matrix products on `par`'s workers from now on; [`Serial`] until
    /// this is called. Any pool gives the same bits.
    pub fn set_parallel(&mut self, par: &'a dyn Parallel) {
        self.par = par;
    }

    pub fn context(&self) -> usize {
        self.context
    }

    /// Runs `token` at the next position of `kv`, filling `logits` (one per
    /// vocabulary entry) when it is given; prompt tokens before the last skip the
    /// output projection, the largest matrix. Every check comes before the cache
    /// changes, so an error leaves it as it was.
    pub fn forward(
        &mut self,
        kv: &mut KvCache,
        token: u32,
        logits: Option<&mut [f32]>,
    ) -> Result<(), Error> {
        let c = self.config;
        if token as usize >= c.vocab {
            return Err(Error::Token(token));
        }
        if logits.as_deref().is_some_and(|l| l.len() != c.vocab) {
            return Err(Error::Logits);
        }
        if !self.fits(kv) {
            return Err(Error::CacheShape);
        }
        let pos = kv.len();
        if pos >= kv.capacity() {
            return Err(Error::ContextFull);
        }
        let embd = self.weights.token_embd;
        embd.dequantize_row(token as usize, &mut self.s.x);
        for layer in 0..c.n_layer {
            self.attention(kv, layer, pos);
            self.feed_forward(layer);
        }
        kv.advance();
        if let Some(logits) = logits {
            let (w, s) = (&self.weights, &mut self.s);
            rms_norm(&s.x, &w.output_norm, c.rms_eps, &mut s.xn);
            s.q_embd.quantize(0, &s.xn);
            run(self.par, self.tier, &s.q_embd, &mut [(&w.output, logits)]);
        }
        Ok(())
    }

    /// Whether `kv` was made for this model: its shape, and no more positions
    /// than the RoPE table covers.
    pub fn fits(&self, kv: &KvCache) -> bool {
        kv.fits(&self.config) && kv.capacity() <= self.context
    }

    fn attention(&mut self, kv: &mut KvCache, layer: usize, pos: usize) {
        let c = self.config;
        let (hd, kv_dim) = (c.head_dim, c.kv_dim());
        let w = &self.weights.layers[layer];
        let s = &mut self.s;
        rms_norm(&s.x, &w.attn_norm, c.rms_eps, &mut s.xn);
        s.q_embd.quantize(0, &s.xn);
        let mut qkv = [(&w.wq, &mut s.q[..]), (&w.wk, &mut s.k), (&w.wv, &mut s.v)];
        run(self.par, self.tier, &s.q_embd, &mut qkv);
        for head in s.q.chunks_exact_mut(hd) {
            rms_norm_in_place(head, &w.q_norm, c.rms_eps);
            self.rope.apply(head, pos);
        }
        for head in s.k.chunks_exact_mut(hd) {
            rms_norm_in_place(head, &w.k_norm, c.rms_eps);
            self.rope.apply(head, pos);
        }
        let (k_slot, v_slot) = kv.slot_mut(layer, pos);
        k_slot.copy_from_slice(&s.k);
        v_slot.copy_from_slice(&s.v);
        let (keys, values) = kv.layer(layer, pos);
        let group = c.n_head / c.n_head_kv;
        let scale = 1.0 / libm::sqrtf(hd as f32);
        let scores = &mut s.scores[..=pos];
        for (h, (q, out)) in
            s.q.chunks_exact(hd)
                .zip(s.att.chunks_exact_mut(hd))
                .enumerate()
        {
            let at = h / group * hd;
            for (t, score) in scores.iter_mut().enumerate() {
                *score = dot(q, &keys[t * kv_dim + at..][..hd]) * scale;
            }
            softmax(scores);
            out.fill(0.0);
            for (t, &p) in scores.iter().enumerate() {
                for (o, &v) in out.iter_mut().zip(&values[t * kv_dim + at..][..hd]) {
                    *o += p * v;
                }
            }
        }
        s.q_att.quantize(0, &s.att);
        run(
            self.par,
            self.tier,
            &s.q_att,
            &mut [(&w.wo, &mut s.out[..])],
        );
        for (x, o) in s.x.iter_mut().zip(&s.out) {
            *x += o;
        }
    }

    fn feed_forward(&mut self, layer: usize) {
        let c = self.config;
        let w = &self.weights.layers[layer];
        let s = &mut self.s;
        rms_norm(&s.x, &w.ffn_norm, c.rms_eps, &mut s.xn);
        s.q_embd.quantize(0, &s.xn);
        let mut gate_up = [(&w.w_gate, &mut s.gate[..]), (&w.w_up, &mut s.up)];
        run(self.par, self.tier, &s.q_embd, &mut gate_up);
        for (g, u) in s.gate.iter_mut().zip(&s.up) {
            *g = *g / (1.0 + libm::expf(-*g)) * u;
        }
        s.q_ff.quantize(0, &s.gate);
        run(
            self.par,
            self.tier,
            &s.q_ff,
            &mut [(&w.w_down, &mut s.out[..])],
        );
        for (x, o) in s.x.iter_mut().zip(&s.out) {
            *x += o;
        }
    }
}

/// An upper bound on the bytes [`Model::new`] allocates for `context` positions
/// besides the norms: the scratch vectors, the activations in both formats and
/// the attention scores.
pub(crate) fn scratch_bytes(c: &Config, context: usize) -> usize {
    let (e, q, kv, ff) = (c.n_embd, c.q_dim(), c.kv_dim(), c.n_ff);
    let f32s = e
        .saturating_mul(3)
        .saturating_add(q.saturating_mul(2))
        .saturating_add(kv.saturating_mul(2))
        .saturating_add(ff.saturating_mul(2))
        .saturating_add(context);
    let acts = Acts::bytes(e, 1)
        .saturating_add(Acts::bytes(q, 1))
        .saturating_add(Acts::bytes(ff, 1));
    f32s.saturating_mul(4).saturating_add(acts)
}

/// Matrices that share the input `x` times its one token, each into its output,
/// in one call on `par`.
fn run<const N: usize>(
    par: &dyn Parallel,
    tier: Tier,
    x: &Acts,
    jobs: &mut [(&Matrix<'_>, &mut [f32]); N],
) {
    let mut items = jobs.each_mut().map(|(m, y)| (x.times(m, 1), &mut **y));
    matvec_parallel(par, tier, &mut items);
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

/// A dot product in eight running sums, so it vectorises while its order stays
/// fixed in the source.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut sums = [0.0f32; 8];
    let (a8, a_tail) = a.as_chunks::<8>();
    let (b8, b_tail) = b.as_chunks::<8>();
    for (a, b) in a8.iter().zip(b8) {
        for i in 0..8 {
            sums[i] += a[i] * b[i];
        }
    }
    for (i, (x, y)) in a_tail.iter().zip(b_tail).enumerate() {
        sums[i] += x * y;
    }
    ((sums[0] + sums[1]) + (sums[2] + sums[3])) + ((sums[4] + sums[5]) + (sums[6] + sums[7]))
}

/// Softmax in place: the largest score is subtracted first, and the sum runs in
/// f64 as in llama.cpp.
fn softmax(scores: &mut [f32]) {
    let max = scores.iter().fold(f32::NEG_INFINITY, |m, &v| m.max(v));
    let mut sum = 0.0f64;
    for v in scores.iter_mut() {
        *v = libm::expf(*v - max);
        sum += f64::from(*v);
    }
    let inv = (1.0 / sum) as f32;
    for v in scores.iter_mut() {
        *v *= inv;
    }
}
