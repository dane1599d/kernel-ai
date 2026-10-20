//! What one decode step reads: every matrix of a qwen3 model, in the groups the
//! engine multiplies together (q, k and v share an input, as do gate and up),
//! taken from a checked GGUF header. `model pin` writes it into the catalog, so
//! `bench quant` can time a model's decode step and `setup` can tell whether the
//! engine runs it, without downloading the model.
//!
//! In the catalog a matrix is `"TYPE INPUTSxOUTPUTS"`, its GGUF dims in order, and
//! layers that read the same matrices are counted once, in the order they first
//! appear: the decode step reads each layer's matrices once, so the order of the
//! layers does not change what it costs.

use gguf::{Header, TensorType};
use serde_json::{Value, json};

/// The matrix types the engine's forward pass runs: all the quant kernels run,
/// since L6's engine part 2. A catalog model with any other type is pinned but
/// never recommended or run.
pub const ENGINE_TYPES: [&str; 3] = ["Q8_0", "Q4_K", "Q6_K"];

/// The matrix types the quant kernels run, so the only ones a catalog model may
/// use; its norms are F32.
pub const KERNEL_TYPES: [TensorType; 3] = [TensorType::Q8_0, TensorType::Q4_K, TensorType::Q6_K];

/// One matrix: its type, and its GGUF dims (inputs, then outputs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mat {
    pub ty: TensorType,
    pub cols: u64,
    pub rows: u64,
}

/// `count` layers that read the same matrices: q, k and v; the attention output;
/// gate and up; down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layers {
    pub count: u64,
    pub groups: Vec<Vec<Mat>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decode {
    /// The embedding table's type: one row of it is read per token.
    pub embedding: TensorType,
    pub layers: Vec<Layers>,
    /// The output projection: `output.weight`, or the embeddings when tied.
    pub output: Mat,
}

impl Decode {
    /// Checks that `header` holds exactly the tensors a qwen3 model of `config`
    /// has, as `infer::check` does but taking any matrix type the kernels run,
    /// and lists what a decode step reads.
    pub fn from_header(header: &Header<'_>, config: &infer::Config) -> Result<Self, String> {
        let (e, q, kv, ff, hd) = (
            config.n_embd as u64,
            config.q_dim() as u64,
            config.kv_dim() as u64,
            config.n_ff as u64,
            config.head_dim as u64,
        );
        let matrix = |name: &str, cols: u64, rows: u64| -> Result<Mat, String> {
            let ty = tensor(header, name, &[cols, rows])?;
            if !KERNEL_TYPES.contains(&ty) {
                return Err(format!(
                    "tensor {name} is {}; the engine runs Q8_0, Q4_K and Q6_K matrices",
                    ty.name()
                ));
            }
            Ok(Mat { ty, cols, rows })
        };
        let vocab = config.vocab as u64;
        let embedding = matrix("token_embd.weight", e, vocab)?;
        let untied = header.tensor("output.weight").is_some();
        let output = match untied {
            true => matrix("output.weight", e, vocab)?,
            false => embedding.clone(),
        };
        let mut layers: Vec<Layers> = Vec::new();
        for i in 0..config.n_layer {
            let name = |part: &str| format!("blk.{i}.{part}.weight");
            let groups = vec![
                vec![
                    matrix(&name("attn_q"), e, q)?,
                    matrix(&name("attn_k"), e, kv)?,
                    matrix(&name("attn_v"), e, kv)?,
                ],
                vec![matrix(&name("attn_output"), q, e)?],
                vec![
                    matrix(&name("ffn_gate"), e, ff)?,
                    matrix(&name("ffn_up"), e, ff)?,
                ],
                vec![matrix(&name("ffn_down"), ff, e)?],
            ];
            for (part, len) in [
                ("attn_norm", e),
                ("attn_q_norm", hd),
                ("attn_k_norm", hd),
                ("ffn_norm", e),
            ] {
                vector(header, &name(part), len)?;
            }
            match layers.iter_mut().find(|l| l.groups == groups) {
                Some(same) => same.count += 1,
                None => layers.push(Layers { count: 1, groups }),
            }
        }
        vector(header, "output_norm.weight", e)?;
        // The embeddings and the final norm, 11 tensors a layer, and the output.
        if header.tensors().len() != 2 + 11 * config.n_layer + usize::from(untied) {
            return Err("the file has tensors a qwen3 model does not".into());
        }
        no_overlap(header)?;
        Ok(Self {
            embedding: embedding.ty,
            layers,
            output,
        })
    }

    /// Every matrix type the model reads.
    pub fn types(&self) -> impl Iterator<Item = TensorType> + '_ {
        let layers = self.layers.iter().flat_map(|l| l.groups.iter().flatten());
        [self.embedding, self.output.ty]
            .into_iter()
            .chain(layers.map(|m| m.ty))
    }

    /// The types the engine's forward pass cannot run yet, each once, by name.
    pub fn types_not_run(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for ty in self.types() {
            if !ENGINE_TYPES.contains(&ty.name()) && !out.contains(&ty.name()) {
                out.push(ty.name());
            }
        }
        out.sort_unstable();
        out
    }

    pub fn to_json(&self) -> Value {
        let mat = |m: &Mat| json!(format!("{} {}x{}", m.ty.name(), m.cols, m.rows));
        let layers: Vec<Value> = self
            .layers
            .iter()
            .map(|l| {
                let groups: Vec<Value> = l
                    .groups
                    .iter()
                    .map(|g| Value::Array(g.iter().map(mat).collect()))
                    .collect();
                json!({"count": l.count, "groups": groups})
            })
            .collect();
        json!({
            "embedding": self.embedding.name(),
            "layers": layers,
            "output": mat(&self.output),
        })
    }

    pub fn from_json(value: &Value) -> Result<Self, String> {
        let bad = || "the catalog's decode list is malformed".to_string();
        let mut layers = Vec::new();
        for layer in value["layers"].as_array().ok_or_else(bad)? {
            let groups = layer["groups"]
                .as_array()
                .ok_or_else(bad)?
                .iter()
                .map(|g| {
                    g.as_array()
                        .ok_or_else(bad)?
                        .iter()
                        .map(parse_mat)
                        .collect()
                })
                .collect::<Result<Vec<Vec<Mat>>, String>>()?;
            let count = layer["count"].as_u64().ok_or_else(bad)?;
            layers.push(Layers { count, groups });
        }
        let decode = Self {
            embedding: type_named(value["embedding"].as_str().ok_or_else(bad)?)?,
            layers,
            output: parse_mat(&value["output"])?,
        };
        decode.check()?;
        Ok(decode)
    }

    /// Every count and size is positive, every type one the kernels run, and every
    /// row a whole number of its type's blocks.
    pub fn check(&self) -> Result<(), String> {
        let shapes_ok = self.layers.iter().flat_map(|l| l.groups.iter().flatten());
        let counts_ok = !self.layers.is_empty()
            && self.layers.iter().all(|l| {
                l.count > 0 && l.groups.len() == 4 && l.groups.iter().all(|g| !g.is_empty())
            });
        if !counts_ok {
            return Err("the catalog's decode list has an empty layer".into());
        }
        for m in shapes_ok.chain([&self.output]) {
            if m.cols == 0 || m.rows == 0 || m.cols % m.ty.block_len() != 0 {
                return Err(format!(
                    "the catalog's decode list has a bad {} matrix",
                    m.ty.name()
                ));
            }
        }
        if !KERNEL_TYPES.contains(&self.embedding) {
            return Err("the catalog's embedding type is not one the kernels run".into());
        }
        Ok(())
    }
}

fn parse_mat(value: &Value) -> Result<Mat, String> {
    let text = value.as_str().unwrap_or_default();
    let parsed = text.split_once(' ').and_then(|(ty, shape)| {
        let (cols, rows) = shape.split_once('x')?;
        Some((ty, cols.parse().ok()?, rows.parse().ok()?))
    });
    let (ty, cols, rows) = parsed.ok_or(format!("{text:?} is not \"TYPE INPUTSxOUTPUTS\""))?;
    let ty = type_named(ty)?;
    if !KERNEL_TYPES.contains(&ty) {
        return Err(format!("the catalog lists a {} matrix", ty.name()));
    }
    Ok(Mat { ty, cols, rows })
}

fn type_named(name: &str) -> Result<TensorType, String> {
    (0..64)
        .filter_map(TensorType::from_id)
        .find(|t| t.name() == name)
        .ok_or(format!("unknown tensor type {name:?}"))
}

/// The type of tensor `name`, which must have exactly `dims`.
fn tensor(header: &Header<'_>, name: &str, dims: &[u64]) -> Result<TensorType, String> {
    let info = header
        .tensor(name)
        .ok_or(format!("the file has no {name}"))?;
    if info.dims() != dims {
        return Err(format!("tensor {name} has the wrong shape"));
    }
    Ok(info.ty)
}

fn vector(header: &Header<'_>, name: &str, len: u64) -> Result<(), String> {
    match tensor(header, name, &[len])? {
        TensorType::F32 => Ok(()),
        other => Err(format!("tensor {name} is {}, not F32", other.name())),
    }
}

/// No two tensors share bytes of the file.
fn no_overlap(header: &Header<'_>) -> Result<(), String> {
    let mut spans: Vec<(u64, u64)> = header
        .tensors()
        .iter()
        .map(|t| (t.offset, t.size))
        .collect();
    spans.sort_unstable();
    for pair in spans.windows(2) {
        let ((start, size), (next, _)) = (pair[0], pair[1]);
        if start.checked_add(size).is_none_or(|end| end > next) {
            return Err("two tensors share bytes of the file".into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
