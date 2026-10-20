//! A naive f32 qwen3 for checking the engine. Every weight is widened to f32 up
//! front by its own Q8_0 and f16 decoding, every step is a plain loop in the
//! obvious order, the RoPE angle is computed afresh for each pair, and the cache
//! is a list of vectors. Only the f16 rounding of an activation's scale is shared
//! with the engine. Activations are rounded to Q8_0 before each matrix, as the
//! engine does, so the two agree to float rounding and no more.

use crate::Config;
use alloc::vec;
use alloc::vec::Vec;
use gguf::Gguf;

/// A matrix as rows of f32, `rows[out][in]`.
type Matrix = Vec<Vec<f32>>;

struct Layer {
    attn_norm: Vec<f32>,
    wq: Matrix,
    wk: Matrix,
    wv: Matrix,
    q_norm: Vec<f32>,
    k_norm: Vec<f32>,
    wo: Matrix,
    ffn_norm: Vec<f32>,
    w_gate: Matrix,
    w_up: Matrix,
    w_down: Matrix,
}

pub struct Reference {
    c: Config,
    embd: Matrix,
    output: Matrix,
    output_norm: Vec<f32>,
    layers: Vec<Layer>,
    /// `keys[layer][position]` and `values[layer][position]`, `kv_dim` long.
    keys: Vec<Vec<Vec<f32>>>,
    values: Vec<Vec<Vec<f32>>>,
}

/// An f16 by its definition: sign, 2^(exponent - 15) and the mantissa.
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let mant = f64::from(bits & 0x3ff);
    let value = match exp {
        0 => mant / 1024.0 * libm::pow(2.0, -14.0),
        31 => panic!("the fixture has no infinite scales"),
        _ => (1.0 + mant / 1024.0) * libm::pow(2.0, f64::from(exp - 15)),
    };
    (sign * value) as f32
}

fn matrix(file: &Gguf<'_>, name: &str) -> Matrix {
    let t = file.tensor(name).unwrap();
    let cols = t.info.dims()[0] as usize;
    let rows = t.info.dims()[1] as usize;
    let mut out = vec![vec![0.0; cols]; rows];
    for (r, row) in out.iter_mut().enumerate() {
        for (i, v) in row.iter_mut().enumerate() {
            let block = &t.data[(r * cols + i) / 32 * 34..][..34];
            let q = block[2 + i % 32] as i8;
            *v = half(u16::from_le_bytes([block[0], block[1]])) * f32::from(q);
        }
    }
    out
}

fn vector(file: &Gguf<'_>, name: &str) -> Vec<f32> {
    let data = file.tensor(name).unwrap().data;
    data.chunks(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// What the engine's matrices see: each block of 32 rounded to steps of
/// max|x| / 127 (that scale itself rounded to f16), ties to even.
fn q8_round(x: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(x.len());
    for block in x.chunks(32) {
        let amax = block.iter().fold(0.0f32, |m, v| m.max(libm::fabsf(*v)));
        let d = quant::f16::to_f32(quant::f16::from_f32(amax / 127.0));
        let id = if amax == 0.0 { 0.0 } else { 127.0 / amax };
        out.extend(
            block
                .iter()
                .map(|v| d * libm::rintf(v * id).clamp(-127.0, 127.0)),
        );
    }
    out
}

fn times(m: &Matrix, x: &[f32]) -> Vec<f32> {
    let x = q8_round(x);
    m.iter()
        .map(|row| row.iter().zip(&x).map(|(a, b)| a * b).sum())
        .collect()
}

fn rms_norm(x: &[f32], w: &[f32], eps: f32) -> Vec<f32> {
    let mean = x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len() as f64;
    let scale = 1.0 / libm::sqrt(mean + f64::from(eps));
    x.iter()
        .zip(w)
        .map(|(v, w)| (f64::from(*v) * scale) as f32 * w)
        .collect()
}

/// RoPE, NeoX pairs, with each angle `pos * base^(-2i / head_dim)` computed anew.
fn rope(head: &mut [f32], pos: usize, base: f32) {
    let half = head.len() / 2;
    for i in 0..half {
        let angle = pos as f64 * libm::pow(f64::from(base), -2.0 * i as f64 / head.len() as f64);
        let (sin, cos) = (libm::sin(angle) as f32, libm::cos(angle) as f32);
        let (a, b) = (head[i], head[i + half]);
        head[i] = a * cos - b * sin;
        head[i + half] = a * sin + b * cos;
    }
}

impl Reference {
    pub fn new(bytes: &[u8]) -> Self {
        let file = Gguf::parse(bytes).unwrap();
        let c = Config::from_gguf(file.header()).unwrap();
        let name = |i: usize, part: &str| alloc::format!("blk.{i}.{part}.weight");
        let layers = (0..c.n_layer)
            .map(|i| Layer {
                attn_norm: vector(&file, &name(i, "attn_norm")),
                wq: matrix(&file, &name(i, "attn_q")),
                wk: matrix(&file, &name(i, "attn_k")),
                wv: matrix(&file, &name(i, "attn_v")),
                q_norm: vector(&file, &name(i, "attn_q_norm")),
                k_norm: vector(&file, &name(i, "attn_k_norm")),
                wo: matrix(&file, &name(i, "attn_output")),
                ffn_norm: vector(&file, &name(i, "ffn_norm")),
                w_gate: matrix(&file, &name(i, "ffn_gate")),
                w_up: matrix(&file, &name(i, "ffn_up")),
                w_down: matrix(&file, &name(i, "ffn_down")),
            })
            .collect();
        let embd = matrix(&file, "token_embd.weight");
        let output = match file.tensor("output.weight") {
            Some(_) => matrix(&file, "output.weight"),
            None => embd.clone(),
        };
        Self {
            c,
            embd,
            output,
            output_norm: vector(&file, "output_norm.weight"),
            layers,
            keys: vec![Vec::new(); c.n_layer],
            values: vec![Vec::new(); c.n_layer],
        }
    }

    /// The logits after `token` at the next position.
    pub fn forward(&mut self, token: u32) -> Vec<f32> {
        let c = self.c;
        let hd = c.head_dim;
        let pos = self.keys[0].len();
        let mut x = self.embd[token as usize].clone();
        for (l, w) in self.layers.iter().enumerate() {
            let xn = rms_norm(&x, &w.attn_norm, c.rms_eps);
            let mut q = times(&w.wq, &xn);
            let mut k = times(&w.wk, &xn);
            let v = times(&w.wv, &xn);
            for head in q.chunks_mut(hd) {
                let normed = rms_norm(head, &w.q_norm, c.rms_eps);
                head.copy_from_slice(&normed);
                rope(head, pos, c.rope_theta);
            }
            for head in k.chunks_mut(hd) {
                let normed = rms_norm(head, &w.k_norm, c.rms_eps);
                head.copy_from_slice(&normed);
                rope(head, pos, c.rope_theta);
            }
            self.keys[l].push(k);
            self.values[l].push(v);
            let mut att = vec![0.0f32; c.q_dim()];
            for h in 0..c.n_head {
                let g = h / (c.n_head / c.n_head_kv);
                let qh = &q[h * hd..(h + 1) * hd];
                let scores: Vec<f64> = self.keys[l]
                    .iter()
                    .map(|k| {
                        let kh = &k[g * hd..(g + 1) * hd];
                        let dot: f64 = qh.iter().zip(kh).map(|(a, b)| f64::from(a * b)).sum();
                        dot / libm::sqrt(hd as f64)
                    })
                    .collect();
                let max = scores.iter().fold(f64::NEG_INFINITY, |m, &s| m.max(s));
                let exps: Vec<f64> = scores.iter().map(|s| libm::exp(s - max)).collect();
                let total: f64 = exps.iter().sum();
                for (e, v) in exps.iter().zip(&self.values[l]) {
                    for i in 0..hd {
                        att[h * hd + i] += (e / total) as f32 * v[g * hd + i];
                    }
                }
            }
            let o = times(&w.wo, &att);
            x.iter_mut().zip(&o).for_each(|(x, o)| *x += o);
            let xn = rms_norm(&x, &w.ffn_norm, c.rms_eps);
            let gate = times(&w.w_gate, &xn);
            let up = times(&w.w_up, &xn);
            let h: Vec<f32> = gate
                .iter()
                .zip(&up)
                .map(|(g, u)| g / (1.0 + libm::expf(-g)) * u)
                .collect();
            let d = times(&w.w_down, &h);
            x.iter_mut().zip(&d).for_each(|(x, d)| *x += d);
        }
        let xn = rms_norm(&x, &self.output_norm, c.rms_eps);
        times(&self.output, &xn)
    }
}
