//! Hostile vocabularies: a tokenizer built from a cut, mutated or adversarial
//! GGUF file either builds or fails with an error, never panics, and allocates no
//! more than a small multiple of the file. Whatever builds encodes any text and
//! decodes any id, and decoding what it encoded gives the text back.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use bpe::chat::Chat;
use bpe::{Error, Kind, MAX_USER_DEFINED, Tokenizer, bytes};
use gguf::write::Writer;
use hostile::XorShift;

const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 16 * 1024;

const SPECIALS: [(&str, i32); 6] = [
    ("<|endoftext|>", 3),
    ("<|im_start|>", 3),
    ("<|im_end|>", 3),
    ("<think>", 4),
    ("</think>", 4),
    ("[PAD9]", 5),
];

fn alpha(text: &[u8]) -> String {
    text.iter().map(|&b| bytes::to_char(b)).collect()
}

/// The 256 byte tokens, one token per merge, then the specials.
fn vocab() -> (Vec<String>, Vec<i32>, Vec<String>) {
    let mut tokens: Vec<String> = (0..=255u8).map(|b| alpha(&[b])).collect();
    let merges: Vec<String> = ["h e", "l l", "he ll", "o \u{120}", "\u{120} w", "hell o"]
        .iter()
        .map(|m| m.to_string())
        .collect();
    for merge in &merges {
        tokens.push(merge.replace(' ', ""));
    }
    let mut types = vec![1; tokens.len()];
    for (text, ty) in SPECIALS {
        tokens.push(text.to_string());
        types.push(ty);
    }
    (tokens, types, merges)
}

fn sample_with(tokens: &[String], types: &[i32], merges: &[String]) -> Vec<u8> {
    let mut w = Writer::new();
    w.string("tokenizer.ggml.model", "gpt2")
        .string("tokenizer.ggml.pre", "qwen2")
        .strings("tokenizer.ggml.tokens", tokens)
        .i32s("tokenizer.ggml.token_type", types)
        .strings("tokenizer.ggml.merges", merges);
    w.finish()
}

fn sample() -> Vec<u8> {
    let (tokens, types, merges) = vocab();
    sample_with(&tokens, &types, &merges)
}

/// Text with the specials' names, byte-alphabet chars, controls and multi-byte
/// chars mixed in, so every path of the pretokenizer and the splitting runs.
fn random_text(rng: &mut XorShift) -> String {
    const PARTS: [&str; 12] = [
        "hello",
        " world",
        "<think>",
        "</think>",
        "<|im_end|>",
        "\n\n",
        "  ",
        "'s",
        "\u{120}",
        "\u{4e2d}\u{6587}",
        "\u{1f600}",
        "12",
    ];
    let mut text = String::new();
    for _ in 0..rng.below(24) {
        match rng.below(3) {
            0 => text.push(char::from(rng.next() as u8)),
            _ => text.push_str(PARTS[rng.below(PARTS.len())]),
        }
    }
    text
}

/// Uses a tokenizer every way a caller can.
fn exercise(tok: &Tokenizer, rng: &mut XorShift) {
    let len = tok.len() as u32;
    let mut out = Vec::new();
    for id in [
        0,
        1,
        len.wrapping_sub(1),
        len,
        len + 1,
        u32::MAX - 1,
        u32::MAX,
    ] {
        let _ = tok.kind(id);
        let _ = tok.token_bytes(id);
        tok.decode(&[id], &mut out);
    }
    let ids: Vec<u32> = (0..64).map(|_| rng.next() as u32 % (len + 8)).collect();
    tok.decode(&ids, &mut out);
    for _ in 0..4 {
        let text = random_text(rng);
        let mut ids = Vec::new();
        tok.encode(&text, &mut ids);
        let mut back = Vec::new();
        tok.decode(&ids, &mut back);
        assert_eq!(back, text.as_bytes(), "{text:?}");
    }
    if let Ok(chat) = Chat::new(tok) {
        let ids = chat.prompt(tok, Some("be brief"), &random_text(rng));
        assert!(ids.iter().all(|&id| id < len));
    }
}

/// Builds the tokenizer from `file`, checks what it allocates, and uses it.
fn check(file: &[u8], rng: &mut XorShift) -> Result<(), Error> {
    let Ok(header) = gguf::Header::parse(file) else {
        return Ok(());
    };
    let (tok, peak) = hostile::peak_alloc(|| Tokenizer::from_gguf(&header));
    let cap = ALLOC_PER_BYTE * file.len() + ALLOC_FLOOR;
    assert!(
        peak <= cap,
        "{peak} bytes allocated for {} bytes",
        file.len()
    );
    exercise(&tok?, rng);
    Ok(())
}

#[test]
fn the_sample_builds_and_round_trips() {
    let mut rng = XorShift::new(7);
    check(&sample(), &mut rng).unwrap();
}

#[test]
fn every_prefix() {
    let file = sample();
    let mut rng = XorShift::new(11);
    hostile::truncations(&file, |cut, prefix| {
        let result = check(prefix, &mut rng);
        assert!(cut < file.len() || result.is_ok());
    });
}

#[test]
fn random_mutations_of_the_file() {
    let file = sample();
    for seed in [3, 0x51ed_270b_27b1_1c2b] {
        let mut rng = XorShift::new(seed ^ 1);
        hostile::fuzz(&file, seed, 3000, |mutated| {
            let _ = check(mutated, &mut rng);
        });
    }
}

/// Mutations below the file format: token texts, types and merges themselves,
/// which a mutated file rarely reaches intact.
#[test]
fn random_vocabularies() {
    let (tokens, types, merges) = vocab();
    let mut rng = XorShift::new(0x2545_f491_4f6c_dd1d);
    let mut built = 0;
    for _ in 0..3000 {
        let (mut tokens, mut types, mut merges) = (tokens.clone(), types.clone(), merges.clone());
        for _ in 0..1 + rng.below(4) {
            match rng.below(6) {
                0 => {
                    let at = rng.below(tokens.len());
                    tokens[at] = random_text(&mut rng);
                }
                1 => {
                    let at = rng.below(types.len());
                    types[at] = rng.below(8) as i32 - 1;
                }
                2 => {
                    let (a, b) = (rng.below(tokens.len()), rng.below(tokens.len()));
                    merges.insert(
                        rng.below(merges.len() + 1),
                        format!("{} {}", tokens[a], tokens[b]),
                    );
                }
                3 => {
                    if !merges.is_empty() {
                        merges.remove(rng.below(merges.len()));
                    }
                }
                4 => {
                    let at = rng.below(tokens.len());
                    tokens.push(tokens[at].clone());
                    types.push(4);
                }
                _ => {
                    let at = rng.below(merges.len().max(1));
                    if let Some(merge) = merges.get_mut(at) {
                        *merge = random_text(&mut rng);
                    }
                }
            }
        }
        built += usize::from(check(&sample_with(&tokens, &types, &merges), &mut rng).is_ok());
    }
    // Enough edits leave a vocabulary that builds for encoding and decoding to run;
    // most random merges join tokens whose join is no token, as they should.
    assert!(built > 300, "{built} of 3000 built");
}

#[test]
fn too_many_user_defined_tokens_are_refused() {
    // `<think>` and `</think>` are user-defined already.
    let (mut tokens, mut types, merges) = vocab();
    for i in 0..MAX_USER_DEFINED - 1 {
        tokens.push(format!("<tag{i}>"));
        types.push(4);
    }
    let file = sample_with(&tokens, &types, &merges);
    let header = gguf::Header::parse(&file).unwrap();
    assert_eq!(
        Tokenizer::from_gguf(&header).err(),
        Some(Error::TooManyUserDefined)
    );
    // At the limit it builds, and text full of tags still encodes.
    tokens.pop();
    types.pop();
    let file = sample_with(&tokens, &types, &merges);
    let header = gguf::Header::parse(&file).unwrap();
    let tok = Tokenizer::from_gguf(&header).unwrap();
    let text = "<tag5>x<tag4093>".repeat(200);
    let mut ids = Vec::new();
    tok.encode(&text, &mut ids);
    assert_eq!(ids.len(), 600);
}

#[test]
fn adversarial_vocabularies() {
    let (tokens, types, merges) = vocab();
    let build = |tokens: &[String], types: &[i32], merges: &[String]| {
        let file = sample_with(tokens, types, merges);
        let header = gguf::Header::parse(&file).unwrap();
        Tokenizer::from_gguf(&header).err()
    };
    // Every token a control token: no byte can be encoded.
    let controls = vec![3; types.len()];
    assert_eq!(
        build(&tokens, &controls, &merges),
        Some(Error::MissingByte(0))
    );
    // Types for fewer tokens than there are.
    assert_eq!(build(&tokens, &types[1..], &merges), Some(Error::TypeCount));
    // A merge whose result is not a token, one of three words, and an empty one.
    for bad in ["x y", "h e l", ""] {
        let mut merges = merges.clone();
        merges.push(bad.to_string());
        assert_eq!(
            build(&tokens, &types, &merges),
            Some(Error::BadMerge(merges.len() - 1)),
            "{bad:?}"
        );
    }
    // Token texts that collide, and a control token spelled like a word: still a
    // tokenizer, and text never becomes the control token.
    let mut tokens2 = tokens.clone();
    let mut types2 = types.clone();
    tokens2.push("hello".to_string());
    types2.push(3);
    tokens2.push("he".to_string());
    types2.push(1);
    let file = sample_with(&tokens2, &types2, &merges);
    let header = gguf::Header::parse(&file).unwrap();
    let tok = Tokenizer::from_gguf(&header).unwrap();
    let mut ids = Vec::new();
    tok.encode("hello", &mut ids);
    assert!(ids.iter().all(|&id| tok.kind(id) != Kind::Control));
    // The token array holding integers instead of strings.
    let mut w = Writer::new();
    w.string("tokenizer.ggml.model", "gpt2")
        .string("tokenizer.ggml.pre", "qwen2")
        .i32s("tokenizer.ggml.tokens", &types)
        .strings("tokenizer.ggml.merges", &merges);
    let file = w.finish();
    let header = gguf::Header::parse(&file).unwrap();
    assert_eq!(
        Tokenizer::from_gguf(&header).err(),
        Some(Error::MissingKey("tokenizer.ggml.tokens"))
    );
}
