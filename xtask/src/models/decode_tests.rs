//! Decode lists from headers: the one written for each catalog model, and every
//! file a decode list must refuse.

use super::*;
use crate::models::catalog::Catalog;
use gguf::write::Writer;

/// A one-layer qwen3 header, 256 wide, with `ty(name)` as each matrix's type.
/// `extra` adds a tensor qwen3 does not have; `clash` puts the attention output
/// on the embeddings' bytes.
fn header(ty: impl Fn(&str) -> TensorType, untied: bool, extra: bool, clash: bool) -> Vec<u8> {
    let tokens = ["a", "b", "c", "d"];
    let mut w = Writer::new();
    w.string("general.architecture", "qwen3")
        .u32("qwen3.block_count", 1)
        .u32("qwen3.context_length", 64)
        .u32("qwen3.embedding_length", 256)
        .u32("qwen3.feed_forward_length", 512)
        .u32("qwen3.attention.head_count", 2)
        .u32("qwen3.attention.head_count_kv", 1)
        .u32("qwen3.attention.key_length", 128)
        .f32("qwen3.attention.layer_norm_rms_epsilon", 1e-6)
        .strings("tokenizer.ggml.tokens", &tokens);
    fn put(w: &mut Writer, name: &str, dims: &[u64], ty: TensorType) {
        let size = gguf::byte_size(ty, dims).unwrap() as usize;
        w.tensor(name, dims, ty, &vec![0; size]);
    }
    put(&mut w, "token_embd.weight", &[256, 4], ty("token_embd"));
    if untied {
        put(&mut w, "output.weight", &[256, 4], ty("output"));
    }
    put(&mut w, "output_norm.weight", &[256], TensorType::F32);
    let mats = [
        ("attn_q", [256, 256]),
        ("attn_k", [256, 128]),
        ("attn_v", [256, 128]),
        ("attn_output", [256, 256]),
        ("ffn_gate", [256, 512]),
        ("ffn_up", [256, 512]),
        ("ffn_down", [512, 256]),
    ];
    for (part, dims) in mats {
        let name = format!("blk.0.{part}.weight");
        if clash && part == "attn_output" {
            w.raw_tensor(&name, &dims, ty(part).id(), 0);
        } else {
            put(&mut w, &name, &dims, ty(part));
        }
    }
    for (part, len) in [
        ("attn_norm", 256),
        ("attn_q_norm", 128),
        ("attn_k_norm", 128),
        ("ffn_norm", 256),
    ] {
        put(
            &mut w,
            &format!("blk.0.{part}.weight"),
            &[len],
            TensorType::F32,
        );
    }
    if extra {
        put(&mut w, "blk.0.attn_q.bias", &[256], TensorType::F32);
    }
    w.finish()
}

fn decode_of(bytes: &[u8]) -> Result<Decode, String> {
    let header = gguf::Header::parse(bytes).map_err(|e| e.to_string())?;
    let config = infer::Config::from_gguf(&header).map_err(|e| e.to_string())?;
    Decode::from_header(&header, &config)
}

/// Q4_K_M's choice, as the pinned files have it: Q6_K for v, down and the
/// embeddings, Q4_K for the rest.
fn q4_k_m(name: &str) -> TensorType {
    match name {
        "attn_v" | "ffn_down" | "token_embd" | "output" => TensorType::Q6_K,
        _ => TensorType::Q4_K,
    }
}

#[test]
fn a_k_quant_file_lists_its_groups_and_waits_for_part_2() {
    let decode = decode_of(&header(q4_k_m, false, false, false)).unwrap();
    let mat = |ty, cols, rows| Mat { ty, cols, rows };
    let (q4, q6) = (TensorType::Q4_K, TensorType::Q6_K);
    let groups = vec![
        vec![mat(q4, 256, 256), mat(q4, 256, 128), mat(q6, 256, 128)],
        vec![mat(q4, 256, 256)],
        vec![mat(q4, 256, 512), mat(q4, 256, 512)],
        vec![mat(q6, 512, 256)],
    ];
    assert_eq!(decode.layers, [Layers { count: 1, groups }]);
    // Tied: the output projection is the embeddings.
    assert_eq!((decode.embedding, &decode.output), (q6, &mat(q6, 256, 4)));
    assert_eq!(decode.types_not_run(), ["Q4_K", "Q6_K"]);
    assert_eq!(Decode::from_json(&decode.to_json()).unwrap(), decode);
    // Untied, with a Q8_0 output of its own.
    let untied = |name: &str| match name {
        "output" => TensorType::Q8_0,
        other => q4_k_m(other),
    };
    let decode = decode_of(&header(untied, true, false, false)).unwrap();
    assert_eq!(decode.output, mat(TensorType::Q8_0, 256, 4));
}

#[test]
fn files_the_kernels_cannot_run_are_refused() {
    let f16 = |name: &str| match name {
        "ffn_up" => TensorType::F16,
        other => q4_k_m(other),
    };
    let error = decode_of(&header(f16, false, false, false)).unwrap_err();
    assert!(error.contains("blk.0.ffn_up.weight is F16"), "{error}");
    let extra = decode_of(&header(q4_k_m, false, true, false)).unwrap_err();
    assert!(extra.contains("does not"), "{extra}");
    let clash = decode_of(&header(q4_k_m, false, false, true)).unwrap_err();
    assert!(clash.contains("share bytes"), "{clash}");
    // A Q5_K tensor never gets as far: gguf has no size rule for it.
    let mut bytes = header(q4_k_m, false, false, false);
    let at = bytes.windows(4).position(|w| w == b"down").unwrap();
    let type_at = at + 4 + ".weight".len() + 4 + 2 * 8;
    bytes[type_at..type_at + 4].copy_from_slice(&13u32.to_le_bytes());
    assert!(decode_of(&bytes).unwrap_err().contains("Q5_K"));
}

#[test]
fn a_bad_decode_list_is_refused() {
    let good = Catalog::load().unwrap().models[1].decode.to_json();
    let with = |pointer: &str, value: serde_json::Value| {
        let mut json = good.clone();
        *json.pointer_mut(pointer).unwrap() = value;
        Decode::from_json(&json)
    };
    assert!(with("/output", "Q6_K 2560x0".into()).is_err());
    assert!(with("/output", "Q4_K 2500x10".into()).is_err());
    assert!(with("/output", "F16 2560x10".into()).is_err());
    assert!(with("/output", "Q4_K 2560 by 10".into()).is_err());
    assert!(with("/embedding", "Q5_K".into()).is_err());
    assert!(with("/layers/0/count", 0.into()).is_err());
    assert!(with("/layers/0/groups/0", serde_json::json!([])).is_err());
    assert!(with("/layers", serde_json::json!([])).is_err());
    assert!(with("/output", "Q6_K 2048x151936".into()).is_ok());
}

/// The committed Qwen3-0.6B list is what its shapes give: every matrix Q8_0,
/// tied, 28 layers; and every catalog list survives a round trip.
#[test]
fn the_catalog_lists_match_the_models() {
    let catalog = Catalog::load().unwrap();
    let small = &catalog.get("qwen3-0.6b").unwrap().decode;
    let mat = |cols, rows| Mat {
        ty: TensorType::Q8_0,
        cols,
        rows,
    };
    let groups = vec![
        vec![mat(1024, 2048), mat(1024, 1024), mat(1024, 1024)],
        vec![mat(2048, 1024)],
        vec![mat(1024, 3072), mat(1024, 3072)],
        vec![mat(3072, 1024)],
    ];
    assert_eq!(small.layers, [Layers { count: 28, groups }]);
    assert_eq!(small.output, mat(1024, 151_936));
    assert!(small.types_not_run().is_empty());
    for entry in &catalog.models {
        assert_eq!(
            Decode::from_json(&entry.decode.to_json()).unwrap(),
            entry.decode
        );
        assert!(entry.decode.layers.iter().map(|l| l.count).sum::<u64>() > 0);
    }
    // gguf's block sizes are the kernels' block sizes.
    assert_eq!(
        TensorType::Q4_K.block_bytes() as usize,
        quant::q4_k::BLOCK_BYTES
    );
    assert_eq!(
        TensorType::Q6_K.block_bytes() as usize,
        quant::q6_k::BLOCK_BYTES
    );
    assert_eq!(TensorType::Q8_0.block_bytes() as usize, quant::BLOCK_BYTES);
    assert_eq!(TensorType::Q4_K.block_len() as usize, quant::SUPER_LEN);
}
