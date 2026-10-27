//! The prompts' bytes do not change when the product's name moves into
//! `kabi::PRODUCT_NAME` (ADR 0009 D8), so that refactor needs no fluency
//! re-run. A rename changes them, and then it does.

/// FNV-1a 64 of `text`: enough to see any byte change.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[test]
fn the_prompts_are_byte_for_byte_the_ones_measured() {
    let prompts = [
        ("ask", fnv(llmproto::chat::ASK_SYSTEM_PROMPT)),
        ("explain", fnv(llmproto::explain::SYSTEM_PROMPT)),
        ("script", fnv(&llmproto::script::system_prompt())),
        ("tune", fnv(llmproto::tune::system_prompt())),
    ];
    assert_eq!(prompts, PINNED);
}

/// Measured on main before the move, at 6e72684.
const PINNED: [(&str, u64); 4] = [
    ("ask", 0x6cd7_a843_98fe_13ae),
    ("explain", 0x76f4_58c1_d4e9_9882),
    ("script", 0x3261_1534_e912_013b),
    ("tune", 0xd0b5_b456_b542_4d74),
];
