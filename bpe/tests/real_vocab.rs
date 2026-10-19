//! Opt-in: the real Qwen3-0.6B vocabulary. Set KAI_QWEN3_GGUF to the model file,
//! or to its first 6 MB or more, and run with `--ignored`.

use bpe::{Kind, Tokenizer};

fn qwen3() -> Tokenizer {
    let path = std::env::var("KAI_QWEN3_GGUF").expect("KAI_QWEN3_GGUF");
    let bytes = std::fs::read(path).unwrap();
    let header = gguf::Header::parse(&bytes).unwrap();
    Tokenizer::from_gguf(&header).unwrap()
}

#[test]
#[ignore = "needs KAI_QWEN3_GGUF"]
fn qwen3_vocabulary_builds_and_merges_to_known_ids() {
    let tok = qwen3();
    assert_eq!(tok.len(), 151_936);
    let count = |kind| {
        (0..tok.len() as u32)
            .filter(|&id| tok.kind(id) == kind)
            .count()
    };
    assert_eq!(count(Kind::Control), 20);
    assert_eq!(count(Kind::UserDefined), 6);
    assert_eq!(count(Kind::Unused), 267);
    let piece = |text: &str| {
        let mut ids = Vec::new();
        tok.encode_piece(text.as_bytes(), &mut ids);
        ids
    };
    for (text, id) in [
        ("Hello", 9707),
        (" world", 1879),
        ("\n", 198),
        ("\n\n", 271),
        ("user", 872),
        ("assistant", 77091),
        ("system", 8948),
        (" helpful", 10950),
        (",", 11),
        ("!", 0),
    ] {
        assert_eq!(piece(text), [id], "{text:?}");
    }
    for (name, id) in [
        ("<|endoftext|>", 151_643),
        ("<|im_start|>", 151_644),
        ("<|im_end|>", 151_645),
        ("<think>", 151_667),
        ("</think>", 151_668),
    ] {
        assert_eq!(tok.special(name), Some(id), "{name}");
    }
    let mut out = Vec::new();
    tok.decode(&piece("caf\u{e9} \u{65e5}\u{672c}"), &mut out);
    assert_eq!(out, "caf\u{e9} \u{65e5}\u{672c}".as_bytes());
}
