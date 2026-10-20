//! Opt-in: the real Qwen3-0.6B header passes every check. Set KAI_QWEN3_GGUF to
//! the model file, or to its first 6 MB or more, and run with `--ignored`.

#[test]
#[ignore = "needs KAI_QWEN3_GGUF"]
fn qwen3_0_6b_passes_the_checks() {
    let path = std::env::var("KAI_QWEN3_GGUF").expect("KAI_QWEN3_GGUF");
    let bytes = std::fs::read(path).unwrap();
    let header = gguf::Header::parse(&bytes).unwrap();
    let config = infer::Config::from_gguf(&header).unwrap();
    assert_eq!(
        (
            config.n_layer,
            config.n_embd,
            config.n_ff,
            config.n_head,
            config.n_head_kv
        ),
        (28, 1024, 3072, 16, 8)
    );
    assert_eq!(
        (config.head_dim, config.vocab, config.context_length),
        (128, 151_936, 40_960)
    );
    assert_eq!((config.rope_theta, config.rms_eps), (1e6, 1e-6));
    infer::check(&header, &config).unwrap();
    // 512 tokens of f32 KV cache: 112 MiB.
    assert_eq!(config.kv_bytes(512), Some(112 << 20));
}
