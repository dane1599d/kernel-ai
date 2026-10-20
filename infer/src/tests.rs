//! The config, the shape checks, the KV cache and the RoPE table, on generated
//! models.

extern crate std;

use crate::fixture::{self, Spec};
use crate::*;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use gguf::write::Writer;
use gguf::{Gguf, Header, TensorType, Value, ValueType};

/// A tensor as the rewrite sees it; set `name` to empty to drop it.
struct Edit {
    name: String,
    dims: Vec<u64>,
    ty: TensorType,
}

/// What to do with one metadata entry.
enum Meta {
    Keep,
    Drop,
    Set(Value<'static>),
}

/// Writes one metadata value of a kind the fixture uses.
fn put(w: &mut Writer, name: &str, value: Value<'_>) {
    match value {
        Value::Str(s) => w.string(name, s),
        Value::U32(v) => w.u32(name, v),
        Value::F32(v) => w.f32(name, v),
        Value::Bool(v) => w.bool(name, v),
        Value::Array(a) if a.element_type() == ValueType::Str => {
            let items: Vec<&str> = a.strs().unwrap().collect();
            w.strings(name, &items)
        }
        Value::Array(a) if a.element_type() == ValueType::I32 => {
            let items: Vec<i32> = a.ints().unwrap().map(|v| v as i32).collect();
            w.i32s(name, &items)
        }
        other => panic!("rewrite cannot copy {other:?}"),
    };
}

/// Copies `bytes` into a new file with `added` metadata first, letting `key`
/// change or drop metadata, `tensor` change or drop tensors, and `extra` add F32
/// tensors at the end.
fn rewrite_with(
    bytes: &[u8],
    added: &[(&str, Value<'static>)],
    key: impl Fn(&str) -> Meta,
    mut tensor: impl FnMut(&mut Edit),
    extra: &[(&str, u64)],
) -> Vec<u8> {
    let file = Gguf::parse(bytes).unwrap();
    let mut w = Writer::new();
    for &(name, value) in added {
        put(&mut w, name, value);
    }
    for (name, value) in file.header().metadata() {
        match key(name) {
            Meta::Drop => {}
            Meta::Keep => put(&mut w, name, *value),
            Meta::Set(v) => put(&mut w, name, v),
        }
    }
    for info in file.header().tensors() {
        let mut edit = Edit {
            name: info.name.to_string(),
            dims: info.dims().to_vec(),
            ty: info.ty,
        };
        tensor(&mut edit);
        if !edit.name.is_empty() {
            let data = file.tensor(info.name).unwrap().data;
            w.tensor(&edit.name, &edit.dims, edit.ty, data);
        }
    }
    for &(name, len) in extra {
        w.tensor(name, &[len], TensorType::F32, &vec![0; len as usize * 4]);
    }
    w.finish()
}

fn rewrite(bytes: &[u8], key: impl Fn(&str) -> Meta, tensor: impl FnMut(&mut Edit)) -> Vec<u8> {
    rewrite_with(bytes, &[], key, tensor, &[])
}

fn keep(_: &str) -> Meta {
    Meta::Keep
}

fn config_of(bytes: &[u8]) -> Result<Config, Error> {
    Config::from_gguf(&Header::parse(bytes).unwrap())
}

fn checked(bytes: &[u8]) -> Result<Config, Error> {
    let header = Header::parse(bytes).unwrap();
    let config = Config::from_gguf(&header)?;
    check(&header, &config)?;
    Ok(config)
}

#[test]
fn the_tiny_fixture_loads() {
    let spec = Spec::tiny();
    let bytes = fixture::build(&spec);
    assert_eq!(
        bytes,
        fixture::build(&spec),
        "the fixture is not deterministic"
    );
    let config = checked(&bytes).unwrap();
    assert_eq!(config.n_layer, spec.n_layer);
    assert_eq!(config.n_embd, spec.n_embd);
    assert_eq!(config.q_dim(), 128);
    assert_eq!(config.kv_dim(), 64);
    assert_eq!(config.vocab, spec.vocab);
    assert_eq!(config.rope_theta, 1e6);
    let file = Gguf::parse(&bytes).unwrap();
    let weights = Weights::bind(&file, &config).unwrap();
    assert_eq!(weights.layers.len(), 2);
    assert_eq!(weights.layers[1].q_norm.len(), 32);
    assert!(core::ptr::eq(weights.output.data, weights.token_embd.data));
    let tok = bpe::Tokenizer::from_gguf(file.header()).unwrap();
    assert_eq!(tok.len(), config.vocab);
}

#[test]
fn an_untied_output_is_used_and_checked() {
    let spec = Spec {
        untied: true,
        ..Spec::tiny()
    };
    let bytes = fixture::build(&spec);
    let config = checked(&bytes).unwrap();
    let file = Gguf::parse(&bytes).unwrap();
    let weights = Weights::bind(&file, &config).unwrap();
    assert!(!core::ptr::eq(weights.output.data, weights.token_embd.data));
    assert_eq!(weights.output.data.len(), weights.token_embd.data.len());
    let output = |edit: fn(&mut Edit)| {
        checked(&rewrite(&bytes, keep, |t| {
            if t.name == "output.weight" {
                edit(t);
            }
        }))
        .unwrap_err()
    };
    let named = |problem| Error::Tensor("output.weight".into(), problem);
    assert_eq!(
        output(|t| t.ty = TensorType::F32),
        named("has the wrong type")
    );
    assert_eq!(
        output(|t| t.dims = vec![64, 383]),
        named("has the wrong shape")
    );
    // An extra tensor is refused whether or not the output is tied.
    let extra = rewrite_with(&bytes, &[], keep, |_| {}, &[("blk.0.attn_q.bias", 128)]);
    let error = checked(&extra).unwrap_err();
    assert_eq!(
        error,
        Error::Tensor("blk.0.attn_q.bias".into(), "is not part of a qwen3 model")
    );
}

#[test]
fn every_misplaced_tensor_is_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let refused = |edit: fn(&mut Edit)| checked(&rewrite(&bytes, keep, edit)).unwrap_err();
    let named = |name: &str, problem| Error::Tensor(name.into(), problem);
    let dropped = refused(|t| {
        if t.name == "blk.1.ffn_up.weight" {
            t.name.clear();
        }
    });
    assert_eq!(dropped, named("blk.1.ffn_up.weight", "missing"));
    let retyped = refused(|t| {
        if t.name == "blk.0.attn_norm.weight" {
            t.ty = TensorType::F16;
        }
    });
    assert_eq!(
        retyped,
        named("blk.0.attn_norm.weight", "has the wrong type")
    );
    let reshaped = refused(|t| {
        if t.name == "blk.0.attn_k.weight" {
            t.dims = vec![64, 128];
        }
    });
    assert_eq!(
        reshaped,
        named("blk.0.attn_k.weight", "has the wrong shape")
    );
    let flat = refused(|t| {
        if t.name == "blk.0.ffn_down.weight" {
            t.dims = vec![96 * 64];
        }
    });
    assert_eq!(flat, named("blk.0.ffn_down.weight", "has the wrong shape"));
    assert!(checked(&rewrite(&bytes, keep, |_| {})).is_ok());
}

#[test]
fn an_unknown_extra_tensor_is_refused() {
    // A bias the forward pass would ignore.
    let bytes = fixture::build(&Spec::tiny());
    let extra = rewrite_with(&bytes, &[], keep, |_| {}, &[("blk.0.attn_q.bias", 128)]);
    let error = checked(&extra).unwrap_err();
    assert_eq!(
        error,
        Error::Tensor("blk.0.attn_q.bias".into(), "is not part of a qwen3 model")
    );
}

/// The config error after setting `key` to `value`, or several keys at once.
fn set_all(bytes: &[u8], pairs: &[(&'static str, Value<'static>)]) -> Result<Config, Error> {
    let edited = rewrite(
        bytes,
        |k| match pairs.iter().find(|(key, _)| *key == k) {
            Some(&(_, value)) => Meta::Set(value),
            None => Meta::Keep,
        },
        |_| {},
    );
    config_of(&edited)
}

/// The config of the file with `added` metadata.
fn config_with(bytes: &[u8], added: &[(&str, Value<'static>)]) -> Result<Config, Error> {
    config_of(&rewrite_with(bytes, added, keep, |_| {}, &[]))
}

#[test]
fn bad_metadata_is_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let set =
        |key: &'static str, value: Value<'static>| set_all(&bytes, &[(key, value)]).unwrap_err();
    let bad = |key: &'static str| Error::BadValue(key);
    assert_eq!(
        set("general.architecture", Value::Str("llama")),
        Error::Architecture
    );
    let eps = "qwen3.attention.layer_norm_rms_epsilon";
    let no_eps = rewrite(
        &bytes,
        |k| if k == eps { Meta::Drop } else { Meta::Keep },
        |_| {},
    );
    assert_eq!(config_of(&no_eps).unwrap_err(), Error::MissingKey(eps));
    for value in [f32::NAN, 2.0, 0.0, -1.0] {
        assert_eq!(set(eps, Value::F32(value)), bad(eps));
    }
    assert_eq!(set(eps, Value::Str("1e-6")), bad(eps));
    assert_eq!(
        set("qwen3.block_count", Value::U32(0)),
        bad("qwen3.block_count")
    );
    let width = "qwen3.embedding_length";
    assert_eq!(set(width, Value::U32(u32::MAX)), bad(width));
    assert_eq!(set(width, Value::U32(48)), bad(width));
    let ff = "qwen3.feed_forward_length";
    assert_eq!(set(ff, Value::U32(100)), bad(ff));
    assert_eq!(
        set("qwen3.context_length", Value::U32(0)),
        bad("qwen3.context_length")
    );
    // An odd head, keys and values of different widths, and queries that are
    // not whole blocks.
    let key_len = "qwen3.attention.key_length";
    let value_len = "qwen3.attention.value_length";
    assert_eq!(set(key_len, Value::U32(33)), bad(value_len));
    let both = |n: u32| {
        set_all(
            &bytes,
            &[(key_len, Value::U32(n)), (value_len, Value::U32(n))],
        )
    };
    assert_eq!(both(33).unwrap_err(), bad(key_len));
    assert_eq!(both(30).unwrap_err(), bad("qwen3.attention.head_count"));
    assert!(both(24).is_ok());
    let odd_heads = fixture::build(&Spec {
        n_head: 3,
        head_dim: 32,
        ..Spec::tiny()
    });
    assert_eq!(
        config_of(&odd_heads).unwrap_err(),
        bad("qwen3.attention.head_count_kv")
    );
}

#[test]
fn head_size_falls_back_to_width_over_heads() {
    // Without key_length the head is n_embd / n_head, as in llama.cpp.
    let bytes = fixture::build(&Spec {
        head_dim: 16,
        ..Spec::tiny()
    });
    let dropped = rewrite(
        &bytes,
        |k| match k {
            "qwen3.attention.key_length" | "qwen3.attention.value_length" => Meta::Drop,
            _ => Meta::Keep,
        },
        |_| {},
    );
    assert_eq!(config_of(&dropped).unwrap().head_dim, 16);
}

#[test]
fn rope_scaling_is_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let scaled = Error::Unsupported("rope scaling");
    let kind = "qwen3.rope.scaling.type";
    assert_eq!(
        config_with(&bytes, &[(kind, Value::Str("yarn"))]),
        Err(scaled.clone())
    );
    assert_eq!(
        config_with(&bytes, &[(kind, Value::Str("linear"))]),
        Err(scaled.clone())
    );
    assert!(config_with(&bytes, &[(kind, Value::Str("none"))]).is_ok());
    for key in ["qwen3.rope.scaling.factor", "qwen3.rope.scale_linear"] {
        assert_eq!(
            config_with(&bytes, &[(key, Value::F32(4.0))]),
            Err(scaled.clone())
        );
        assert!(config_with(&bytes, &[(key, Value::F32(1.0))]).is_ok());
        assert!(config_with(&bytes, &[(key, Value::F32(0.0))]).is_ok());
    }
}

#[test]
fn the_kv_cache_is_sized_to_the_cap() {
    let config = config_of(&fixture::build(&Spec::tiny())).unwrap();
    let kv = KvCache::new(&config, 100).unwrap();
    assert_eq!(kv.capacity(), 100);
    assert!(kv.is_empty());
    assert_eq!(config.kv_bytes(100), Some(2 * 2 * 64 * 100 * 4));
    assert_eq!(Some(kv.bytes()), config.kv_bytes(100));
    assert_eq!(KvCache::new(&config, 0).err(), Some(Error::Context));
    assert_eq!(KvCache::new(&config, 513).err(), Some(Error::Context));
    // The real 0.6B: 512 tokens of f32 context take 112 MiB.
    let real = Config {
        n_layer: 28,
        n_embd: 1024,
        n_ff: 3072,
        n_head: 16,
        n_head_kv: 8,
        head_dim: 128,
        vocab: 151_936,
        context_length: 40_960,
        rope_theta: 1e6,
        rms_eps: 1e-6,
    };
    assert_eq!(real.kv_bytes(512), Some(112 << 20));
    assert_eq!(real.kv_bytes(usize::MAX), None);
}

#[test]
fn rope_rotates_pairs_half_a_head_apart() {
    let config = config_of(&fixture::build(&Spec::tiny())).unwrap();
    let rope = Rope::new(&config, 8).unwrap();
    // Position 0 is no rotation at all.
    let mut head: Vec<f32> = (0..32).map(|i| i as f32).collect();
    let before = head.clone();
    rope.apply(&mut head, 0);
    assert_eq!(head, before);
    // Pair 0 turns by `pos` radians; the norm of each pair is kept.
    let mut head = vec![0.0f32; 32];
    head[0] = 1.0;
    rope.apply(&mut head, 3);
    assert!((head[0] - libm::cosf(3.0)).abs() < 1e-6);
    assert!((head[16] - libm::sinf(3.0)).abs() < 1e-6);
    let mut head: Vec<f32> = (0..32).map(|i| 1.0 + i as f32 / 10.0).collect();
    let norms: Vec<f32> = (0..16).map(|i| head[i].hypot(head[i + 16])).collect();
    rope.apply(&mut head, 7);
    for i in 0..16 {
        assert!((head[i].hypot(head[i + 16]) - norms[i]).abs() < 1e-5);
    }
    // The last pair turns slowest: theta^(-30/32) of a radian per position.
    let slowest = libm::powf(1e6, -30.0 / 32.0);
    let mut head = vec![0.0f32; 32];
    head[15] = 1.0;
    rope.apply(&mut head, 1);
    assert!((head[31] - libm::sinf(slowest)).abs() < 1e-6);
}

#[test]
fn partial_rotary_embeddings_are_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let dims = "qwen3.rope.dimension_count";
    let partial = config_with(&bytes, &[(dims, Value::U32(16))]);
    assert_eq!(
        partial,
        Err(Error::Unsupported("partial rotary embeddings"))
    );
    assert!(config_with(&bytes, &[(dims, Value::U32(32))]).is_ok());
    let attn = "qwen3.rope.scaling.attn_factor";
    assert_eq!(
        config_with(&bytes, &[(attn, Value::F32(2.0))]),
        Err(Error::Unsupported("rope scaling"))
    );
    assert!(config_with(&bytes, &[(attn, Value::F32(1.0))]).is_ok());
}

#[test]
fn unknown_metadata_and_shared_bytes_are_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let rotated = config_with(&bytes, &[("prism.hadamard.version", Value::U32(1))]);
    assert_eq!(
        rotated,
        Err(Error::UnknownKey("prism.hadamard.version".into()))
    );
    let split = config_with(&bytes, &[("qwen3.expert_count", Value::U32(8))]);
    assert_eq!(split, Err(Error::UnknownKey("qwen3.expert_count".into())));
    assert!(config_with(&bytes, &[("quantize.imatrix.file", Value::Str("x"))]).is_ok());
    // A hostile key is shortened and escaped before it is printed.
    let long = config_with(&bytes, &[("\u{1b}[2J".repeat(40).leak(), Value::U32(1))]);
    let Err(Error::UnknownKey(shown)) = long else {
        panic!("{long:?}");
    };
    assert!(shown.len() < 80 && !shown.contains('\u{1b}'), "{shown}");
    // A tokenizer that names more tokens than the embeddings have rows.
    let names: Vec<String> = (0..400).map(|i| alloc::format!("t{i}")).collect();
    let mut w = Writer::new();
    w.strings("tokenizer.ggml.tokens", &names);
    let source: &'static [u8] = w.finish().leak();
    let longer = Header::parse(source)
        .unwrap()
        .get("tokenizer.ggml.tokens")
        .copied()
        .unwrap();
    let edited = rewrite(
        &bytes,
        |k| match k {
            "tokenizer.ggml.tokens" => Meta::Set(longer),
            _ => Meta::Keep,
        },
        |_| {},
    );
    let header = Header::parse(&edited).unwrap();
    let config = Config::from_gguf(&header).unwrap();
    assert_eq!(
        check(&header, &config),
        Err(Error::BadValue("tokenizer.ggml.tokens"))
    );
}

#[test]
fn tensors_may_not_share_bytes() {
    let bytes = fixture::build(&Spec::tiny());
    let file = Gguf::parse(&bytes).unwrap();
    let mut w = Writer::new();
    for (name, value) in file.header().metadata() {
        put(&mut w, name, *value);
    }
    // Every tensor at offset 0 of one blob big enough for the largest.
    for info in file.header().tensors() {
        w.raw_tensor(info.name, info.dims(), info.ty.id(), 0);
    }
    let mut shared = w.finish();
    shared.resize(shared.len() + 64 * 1024, 0);
    let header = Header::parse(&shared).unwrap();
    let config = Config::from_gguf(&header).unwrap();
    assert_eq!(check(&header, &config), Err(Error::Overlap));
}
