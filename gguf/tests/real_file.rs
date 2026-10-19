//! Opt-in: the header of the real Qwen3-0.6B Q8_0 file. Set KAI_QWEN3_GGUF to the
//! file, or to its first 6 MB or more, and run with `--ignored`.

use gguf::{Header, TensorType};

#[test]
#[ignore = "needs KAI_QWEN3_GGUF"]
fn qwen3_0_6b_header() {
    let path = std::env::var("KAI_QWEN3_GGUF").expect("KAI_QWEN3_GGUF");
    let bytes = std::fs::read(path).unwrap();
    let header = Header::parse(&bytes).unwrap();
    assert_eq!(header.metadata().count(), 28);
    assert_eq!(
        header.get("general.architecture").unwrap().as_str(),
        Some("qwen3")
    );
    let tokens = header
        .get("tokenizer.ggml.tokens")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(tokens.len(), 151_936);
    assert_eq!(header.tensors().len(), 310);
    let q8 = header
        .tensors()
        .iter()
        .filter(|t| t.ty == TensorType::Q8_0)
        .count();
    assert_eq!(q8, 197);
    let embd = header.tensor("token_embd.weight").unwrap();
    assert_eq!(embd.dims(), [1024, 151_936]);
    assert!(header.tensor("output.weight").is_none());
    assert!(header.data_offset() < 6_000_000);
    // The full file is 639,446,688 bytes.
    assert_eq!(header.check_extents(639_446_688), Ok(()));
}
