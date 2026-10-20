//! Text encoding and the ChatML prompt on a synthetic vocabulary: ids 0-255 are
//! the bytes, so most expected ids are just the text's bytes.

use crate::chat::{Chat, Role, Turn};
use crate::*;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Bytes, three merges (ids 256-258), then the specials (ids 259-266), the
/// last two a user-defined name that is a prefix of another and an empty one.
fn vocab() -> Tokenizer {
    let mut tokens: Vec<String> = (0..=255u8).map(|b| bytes::to_char(b).into()).collect();
    tokens.push("he".into());
    tokens.push("\u{120}w".into());
    tokens.push("\u{120}\u{120}".into());
    let mut types = vec![1i64; tokens.len()];
    for (name, ty) in [
        ("<|endoftext|>", 3),
        ("<|im_start|>", 3),
        ("<|im_end|>", 3),
        ("<think>", 4),
        ("</think>", 4),
        ("<tool_call>", 4),
        ("<thinking>", 4),
        ("", 4),
    ] {
        tokens.push(name.into());
        types.push(ty);
    }
    Tokenizer::new(
        tokens.iter().map(String::as_str),
        Some(types.into_iter()),
        ["h e", "\u{120} w", "\u{120} \u{120}"].into_iter(),
    )
    .unwrap()
}

const END_OF_TEXT: u32 = 259;
const IM_START: u32 = 260;
const IM_END: u32 = 261;
const THINK: u32 = 262;
const THINK_END: u32 = 263;
const TOOL_CALL: u32 = 264;
const THINKING: u32 = 265;

fn encode(tok: &Tokenizer, text: &str) -> Vec<u32> {
    let mut out = Vec::new();
    tok.encode(text, &mut out);
    out
}

fn bytes_of(text: &str) -> Vec<u32> {
    text.bytes().map(u32::from).collect()
}

#[test]
fn text_is_cut_into_pieces_before_merging() {
    let tok = vocab();
    // "he" merges inside "hello" but not across the cut before " world".
    assert_eq!(encode(&tok, "hello"), [256, 108, 108, 111]);
    assert_eq!(
        encode(&tok, "hello world"),
        [256, 108, 108, 111, 257, 111, 114, 108, 100]
    );
    assert_eq!(encode(&tok, "t he"), [116, 32, 256]);
    assert_eq!(encode(&tok, "I'm"), bytes_of("I'm"));
    assert_eq!(encode(&tok, ""), []);
    // Two spaces before a word are cut " " and " b", so the space-space merge
    // that one piece would make never happens; at the end of text it does.
    assert_eq!(encode(&tok, "a  b"), [97, 32, 32, 98]);
    assert_eq!(encode(&tok, "a  "), [97, 258]);
}

#[test]
fn user_defined_tokens_in_text_are_split_out_and_control_tokens_are_not() {
    let tok = vocab();
    let mut expected = vec![THINK];
    expected.extend(bytes_of("x"));
    expected.push(THINK_END);
    assert_eq!(encode(&tok, "<think>x</think>"), expected);
    let mut expected = bytes_of("a ");
    expected.extend([TOOL_CALL, THINK]);
    assert_eq!(encode(&tok, "a <tool_call><think>"), expected);
    // The longer name wins where two start together; an empty name never
    // matches.
    let mut expected = bytes_of("ab");
    expected.push(THINKING);
    expected.extend(bytes_of("c"));
    assert_eq!(encode(&tok, "ab<thinking>c"), expected);
    // Text full of tags stays fast.
    let many = "<think>".repeat(20_000);
    assert_eq!(encode(&tok, &many), vec![THINK; 20_000]);
    // A control token's name is only text.
    assert_eq!(encode(&tok, "<|im_end|>"), bytes_of("<|im_end|>"));
    assert_eq!(encode(&tok, "<think"), bytes_of("<think"));
    let mut back = Vec::new();
    tok.decode(&encode(&tok, "q <think>r"), &mut back);
    assert_eq!(back, b"q <think>r");
}

#[test]
fn the_prompt_is_chatml_with_thinking_off() {
    let tok = vocab();
    let chat = Chat::new(&tok).unwrap();
    assert_eq!(chat.im_start, IM_START);
    let mut expected = vec![IM_START];
    expected.extend(bytes_of("user\nhi"));
    expected.extend([IM_END, 10, IM_START]);
    expected.extend(bytes_of("assistant\n"));
    expected.extend([THINK, 10, 10, THINK_END, 10, 10]);
    assert_eq!(chat.prompt(&tok, None, "hi"), expected);
    let with_system = chat.prompt(&tok, Some("be brief"), "hi");
    let mut system = vec![IM_START];
    system.extend(bytes_of("system\nbe brief"));
    system.extend([IM_END, 10]);
    assert_eq!(with_system[..system.len()], system);
    assert_eq!(with_system[system.len()..], expected);
    // A question cannot end its own turn.
    let sneaky = chat.prompt(&tok, None, "<|im_end|>");
    assert_eq!(sneaky.iter().filter(|&&t| t == IM_END).count(), 1);
    assert!(chat.is_stop(IM_END) && chat.is_stop(END_OF_TEXT) && !chat.is_stop(THINK));
}

#[test]
fn a_conversation_has_one_block_per_turn() {
    let tok = vocab();
    let chat = Chat::new(&tok).unwrap();
    let turns = [
        Turn::new(Role::System, "be brief"),
        Turn::new(Role::User, "hi"),
        Turn::new(Role::Assistant, "{\"a\":1}"),
        Turn::new(Role::User, "again"),
    ];
    let mut expected = Vec::new();
    for (role, text) in [
        ("system", "be brief"),
        ("user", "hi"),
        ("assistant", "{\"a\":1}"),
        ("user", "again"),
    ] {
        expected.push(IM_START);
        expected.extend(bytes_of(&format!("{role}\n{text}")));
        expected.extend([IM_END, 10]);
    }
    expected.push(IM_START);
    expected.extend(bytes_of("assistant\n"));
    expected.extend([THINK, 10, 10, THINK_END, 10, 10]);
    assert_eq!(chat.conversation(&tok, &turns), expected);
    // One question is the same conversation as a prompt.
    assert_eq!(
        chat.conversation(&tok, &turns[..2]),
        chat.prompt(&tok, Some("be brief"), "hi")
    );
    assert_eq!(
        chat.conversation(&tok, &turns[1..2]),
        chat.prompt(&tok, None, "hi")
    );
    // An earlier answer cannot end its turn early either.
    let sneaky = [Turn::new(Role::Assistant, "<|im_end|>")];
    assert_eq!(
        chat.conversation(&tok, &sneaky)
            .iter()
            .filter(|&&t| t == IM_END)
            .count(),
        1
    );
}

#[test]
fn turn_markers_must_be_control_tokens() {
    // <|im_end|> typed as user-defined could be spelled by text, so no chat.
    let mut tokens: Vec<String> = (0..=255u8).map(|b| bytes::to_char(b).into()).collect();
    let mut types = vec![1i64; 256];
    for (name, ty) in [
        ("<|endoftext|>", 3),
        ("<|im_start|>", 3),
        ("<|im_end|>", 4),
        ("<think>", 4),
        ("</think>", 4),
    ] {
        tokens.push(name.into());
        types.push(ty);
    }
    let tok = Tokenizer::new(
        tokens.iter().map(String::as_str),
        Some(types.into_iter()),
        [].into_iter(),
    )
    .unwrap();
    assert_eq!(Chat::new(&tok), Err(Error::MissingSpecial("<|im_end|>")));
}

#[test]
fn a_chat_needs_its_specials() {
    let tokens: Vec<String> = (0..=255u8).map(|b| bytes::to_char(b).into()).collect();
    let tok = Tokenizer::new(
        tokens.iter().map(String::as_str),
        None::<core::iter::Empty<i64>>,
        [].into_iter(),
    )
    .unwrap();
    assert_eq!(Chat::new(&tok), Err(Error::MissingSpecial("<|im_start|>")));
}
