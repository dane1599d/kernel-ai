//! The JSON shape: `need` against a search of every state, the bytes the mask
//! takes, stops at and refuses, and every token it allows on random
//! vocabularies, logits and budgets. Whole answers from generated models are in
//! `reply_tests`.

use crate::json::{
    Field, Grammar, Limit, Overflow, Shape, State, Stop, ascii_tokens, spells_ascii,
};
use crate::{Error, Sampler};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bpe::Tokenizer;

/// The shape scripts take, with llmproto's limits: its reader trims the end of
/// the script and both ends of the summary.
pub(crate) const SCRIPT: [Field; 2] = [
    Field {
        name: "script",
        max_chars: 3072,
        max_lines: 40,
        max_line_chars: 120,
        trim_start: false,
        hard: false,
    },
    Field {
        name: "summary",
        max_chars: 100,
        max_lines: 1,
        max_line_chars: 100,
        trim_start: true,
        hard: false,
    },
];
pub(crate) const SCRIPT_SHAPE: Shape<'static> = Shape {
    fields: &SCRIPT,
    choices: &[],
};

/// Small limits, so every state can be searched.
const SMALL: [Field; 2] = [
    Field {
        name: "a",
        max_chars: 6,
        max_lines: 3,
        max_line_chars: 3,
        trim_start: false,
        hard: false,
    },
    Field {
        name: "b_1",
        max_chars: 4,
        max_lines: 2,
        max_line_chars: 3,
        trim_start: true,
        hard: false,
    },
];

const PRINTABLE: core::ops::RangeInclusive<u8> = b' '..=b'~';

/// Every state reachable from the start, and each one's true distance to the
/// closed object over the printable bytes it takes.
fn distances(grammar: &Grammar<'_>) -> BTreeMap<State, Option<usize>> {
    let mut seen = BTreeMap::new();
    let mut queue = vec![grammar.start()];
    while let Some(state) = queue.pop() {
        if seen.insert(state, None).is_some() {
            continue;
        }
        queue.extend(PRINTABLE.filter_map(|b| grammar.step(state, b).ok()));
    }
    loop {
        let mut changed = false;
        let states: Vec<State> = seen.keys().copied().collect();
        for state in states {
            let best = match state.is_done() {
                true => Some(0),
                false => PRINTABLE
                    .filter_map(|b| grammar.step(state, b).ok())
                    .filter_map(|next| seen[&next].map(|d| d + 1))
                    .min(),
            };
            if best != seen[&state] {
                seen.insert(state, best);
                changed = true;
            }
        }
        if !changed {
            return seen;
        }
    }
}

#[test]
fn need_is_the_true_distance_from_every_state() {
    let mirrored = [SMALL[1], SMALL[0]];
    for fields in [&SMALL[..], &SMALL[1..], &SMALL[..1], &mirrored[..]] {
        let grammar = Grammar::new(&Shape {
            fields,
            choices: &[],
        })
        .unwrap();
        let states = distances(&grammar);
        assert!(states.len() > 15, "{}", states.len());
        for (state, distance) in &states {
            // Every state the shape takes can still close.
            assert_eq!(Some(grammar.need(state)), *distance, "{state:?}");
        }
        // From the start: `{"a":"` and `","b":"` before the strings, and `"}`.
        let leads: usize = fields.iter().map(|f| f.name.len() + 6).sum::<usize>();
        assert_eq!(grammar.need(&grammar.start()), leads - 1 + 2);
    }
}

#[test]
fn the_object_and_its_strings_are_exact() {
    let grammar = Grammar::new(&SCRIPT_SHAPE).unwrap();
    let start = grammar.start();
    assert_eq!(grammar.need(&start), 26);
    let text = br#"{"script":"print(\"hi\");\nlet a = 1;","summary":"Says hi \/ sets a."}"#;
    let done = grammar.advance(start, text).unwrap();
    assert!(done.is_done());
    assert_eq!(grammar.need(&done), 0);
    // Strings may be empty or blank, as a server's may: the reader decides.
    for empty in [
        &br#"{"script":"","summary":""}"#[..],
        br#"{"script":"  ","summary":" "}"#,
    ] {
        assert!(grammar.advance(start, empty).unwrap().is_done());
    }
    // A \u escape, built so no tool or editor reads it as one.
    let unicode = [&br#"{"script":"a\"#[..], b"u0041"].concat();
    for bad in [
        &br#"{ "script""#[..],
        br#"{"summary""#,
        br#"{"script":"a"}"#,
        br#"{"script":"a\tb"#,
        &unicode,
        b"{\"script\":\"a\x01",
        b"{\"script\":\"a\n",
        b"{\"script\":\"\xc3\xa9",
        br#"{"script":"a","summary":"x\ty"#,
        br#"{"script":"a","summary":"x"}}"#,
        br#"{"script":"a","summary":"x"} "#,
    ] {
        assert_eq!(
            grammar.advance(start, bad),
            Err(Stop::Masked),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn a_token_past_a_limit_stops_and_says_which() {
    let grammar = Grammar::new(&SCRIPT_SHAPE).unwrap();
    let at = |bytes: &[u8]| grammar.advance(grammar.start(), bytes);
    let over = |field, limit, line| Err(Stop::Over(Overflow { field, limit, line }));
    let script = |text: &[u8]| [&br#"{"script":""#[..], text].concat();
    // The 121st character of a line, plain or escaped; a full line still takes a
    // newline or the closing quote.
    let line = [b'x'; 120];
    let full = at(&script(&line)).unwrap();
    assert_eq!(grammar.step(full, b'x'), over(0, Limit::LineChars, 1));
    assert_eq!(
        grammar.advance(full, br#"\""#),
        over(0, Limit::LineChars, 1)
    );
    assert!(grammar.advance(full, br"\nx").is_ok());
    assert!(grammar.step(full, b'"').is_ok());
    // A 41st line; a newline that ends the script is trimmed, so it may come.
    let forty = at(&script(&br"x\n".repeat(39))).unwrap();
    assert_eq!(grammar.advance(forty, br"\nx"), over(0, Limit::Lines, 40));
    assert!(grammar.step(forty, b'x').is_ok());
    let ended = grammar.advance(forty, br#"x\n  \n""#).unwrap();
    assert!(
        grammar
            .advance(ended, br#","summary":"ok"}"#)
            .unwrap()
            .is_done()
    );
    // The 3,073rd character: 30 lines of 100 and a newline, then 42 more.
    let lines = [&[b'y'; 100][..], br"\n"].concat().repeat(30);
    let full = at(&script(&[&lines[..], &[b'z'; 42][..]].concat())).unwrap();
    assert_eq!(30 * 101 + 42, 3072);
    assert_eq!(grammar.step(full, b'z'), over(0, Limit::Chars, 31));
    assert_eq!(grammar.advance(full, br#"\""#), over(0, Limit::Chars, 31));
    assert_eq!(grammar.advance(full, br" \n  x"), over(0, Limit::Chars, 31));
    assert!(grammar.advance(full, br#" \n  ""#).is_ok());
    // Spaces past a full line are over only if more than spaces follows.
    let line = at(&script(&[b'x'; 120])).unwrap();
    assert_eq!(grammar.advance(line, b"  x"), over(0, Limit::LineChars, 1));
    assert_eq!(
        grammar.advance(line, br"  \nx"),
        over(0, Limit::LineChars, 1)
    );
    assert!(grammar.advance(line, br#"  ""#).is_ok());
    // The summary's 101st character, and a newline inside it; spaces and
    // newlines around it count for nothing, as the reader trims them.
    let summary = br#"{"script":"a","summary":""#.to_vec();
    assert_eq!(
        at(&[&summary[..], br"a\nb"].concat()),
        over(1, Limit::Lines, 1)
    );
    let padded = [&summary[..], br"\n  ", &[b's'; 100], br#" \n""#].concat();
    assert!(at(&padded).is_ok());
    let full = at(&[&summary[..], &[b's'; 100]].concat()).unwrap();
    assert_eq!(grammar.step(full, b's'), over(1, Limit::Chars, 1));
    assert_eq!(grammar.advance(full, b" s"), over(1, Limit::Chars, 1));
    assert!(grammar.advance(full, br#""}"#).unwrap().is_done());
}

#[test]
fn shapes_that_cannot_be_decoded_are_refused() {
    let field = SCRIPT[0];
    for (fields, why) in [
        (&[][..], "has no fields"),
        (
            &[Field {
                name: "a b",
                ..field
            }][..],
            "has a field name that is not a plain word",
        ),
        (
            &[Field { name: "", ..field }][..],
            "has a field name that is not a plain word",
        ),
        (&[field, field][..], "has two fields of one name"),
        (
            &[Field {
                max_line_chars: 0,
                ..field
            }][..],
            "has a string with no room for a character",
        ),
    ] {
        assert_eq!(
            Grammar::new(&Shape {
                fields,
                choices: &[]
            })
            .err(),
            Some(Error::Shape(why))
        );
    }
}

/// A vocabulary of the 256 bytes, `extra` longer tokens and the chat specials.
pub(crate) fn vocabulary(extra: &[&[u8]]) -> Tokenizer {
    let to_text =
        |bytes: &[u8]| -> String { bytes.iter().map(|&b| bpe::bytes::to_char(b)).collect() };
    let mut tokens: Vec<String> = (0..=255u8).map(|b| to_text(&[b])).collect();
    for &bytes in extra {
        let text = to_text(bytes);
        if !tokens.contains(&text) {
            tokens.push(text);
        }
    }
    let mut types = vec![1i64; tokens.len()];
    for name in ["<|im_end|>", "<|im_start|>"] {
        tokens.push(name.into());
        types.push(3);
    }
    Tokenizer::new(
        tokens.iter().map(String::as_str),
        Some(types.into_iter()),
        [].into_iter(),
    )
    .unwrap()
}

/// splitmix64, for random tokens, logits and budgets.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// What a closed answer must be: the shape's fields in order, each a string of
/// printable ASCII and newlines within its limits once trimmed as the reader
/// trims it.
pub(crate) fn assert_fits(shape: &Shape<'_>, text: &[u8]) {
    let shown = String::from_utf8_lossy(text);
    let value: serde_json::Value = serde_json::from_slice(text).expect(&shown);
    let object = value.as_object().expect(&shown);
    assert_eq!(object.len(), shape.fields.len(), "{shown}");
    for field in shape.fields {
        let raw = object[field.name].as_str().expect(&shown);
        assert!(
            raw.chars().all(|c| c == '\n' || (' '..='~').contains(&c)),
            "{shown}"
        );
        let blank = |c: char| c == ' ' || c == '\n';
        let string = match field.trim_start {
            true => raw.trim_matches(blank),
            false => raw.trim_end_matches(blank),
        };
        assert!(string.chars().count() <= field.max_chars, "{shown}");
        assert!(string.matches('\n').count() < field.max_lines, "{shown}");
        assert!(
            string
                .split('\n')
                .all(|line| line.chars().count() <= field.max_line_chars),
            "{shown}"
        );
    }
    let order: Vec<&str> = shape.fields.iter().map(|f| f.name).collect();
    assert_eq!(keys_in_order(text), order, "{shown}");
}

/// The keys of a flat object of strings, in the order written.
fn keys_in_order(text: &[u8]) -> Vec<&str> {
    let mut keys = Vec::new();
    let (mut at, mut key) = (1, true);
    while at < text.len() && text[at] == b'"' {
        let start = at + 1;
        at = start;
        while text[at] != b'"' {
            at += if text[at] == b'\\' { 2 } else { 1 };
        }
        if key {
            keys.push(core::str::from_utf8(&text[start..at]).unwrap());
        }
        // Past the quote and the `:` or `,` after it.
        at += 2;
        key = !key;
    }
    keys
}

/// Pieces a real vocabulary has, the JSON ones included, and random ones.
fn pieces(rng: &mut Rng) -> Vec<Vec<u8>> {
    let mut pieces: Vec<Vec<u8>> = [
        &b"\"}"[..],
        b"\":\"",
        b"\",\"",
        b"{\"",
        b"\\n",
        b"\\\"",
        b"\\\\",
        b"print",
        b"(\"",
        b"\");",
        b"    ",
        b"script",
        b"summary",
        b"\"}\n",
        b"\\u00e9",
    ]
    .iter()
    .map(|piece| piece.to_vec())
    .collect();
    for _ in 0..300 {
        let len = 2 + rng.below(6);
        pieces.push(
            (0..len)
                .map(|_| match rng.below(10) {
                    0 => b'"',
                    1 => b'\\',
                    2 => b' ',
                    3 => b'\n',
                    _ => b' ' + rng.below(95) as u8,
                })
                .collect(),
        );
    }
    pieces
}

#[test]
fn every_answer_closes_stops_at_a_limit_or_is_cut_off() {
    let mut rng = Rng(7);
    let extra = pieces(&mut rng);
    let extra: Vec<&[u8]> = extra.iter().map(Vec::as_slice).collect();
    let tok = vocabulary(&extra);
    let ascii = ascii_tokens(&tok).unwrap();
    assert!(spells_ascii(&tok, &ascii));
    assert!(ascii.len() < tok.len());
    let mut candidates = Vec::new();
    let mut endings = [0usize; 3];
    for shape in [
        SCRIPT_SHAPE,
        Shape {
            fields: &SMALL,
            choices: &[],
        },
    ] {
        let grammar = Grammar::new(&shape).unwrap();
        let least = grammar.need(&grammar.start());
        for round in 0..300 {
            let budget = least + rng.below(4 + round % 50 * 8);
            let mut sampler = Sampler::seeded(1.0 + (round % 3) as f32, round as u64);
            let (mut state, mut text) = (grammar.start(), Vec::new());
            let mut logits = vec![0.0f32; tok.len()];
            let mut ending = 2;
            for made in 1..=budget {
                for logit in logits.iter_mut() {
                    *logit = (rng.below(2001) as f32 - 1000.0) / 100.0;
                }
                grammar.allowed(&state, &tok, &ascii, &mut candidates);
                let token = sampler.pick_from(&logits, &candidates).unwrap();
                match grammar.advance(state, tok.token_bytes(token)) {
                    Ok(next) => state = next,
                    Err(Stop::Over(overflow)) => {
                        assert!(overflow.field < shape.fields.len());
                        ending = 1;
                        break;
                    }
                    Err(Stop::Masked) => panic!("the mask let a token through it refuses"),
                }
                text.extend_from_slice(tok.token_bytes(token));
                if state.is_done() {
                    ending = 0;
                    break;
                }
                if grammar.need(&state) > budget - made {
                    break;
                }
            }
            endings[ending] += 1;
            if ending == 0 {
                assert_fits(&shape, &text);
            }
        }
    }
    // Every way of ending came up.
    assert!(endings.iter().all(|&n| n > 10), "{endings:?}");
}

#[test]
fn every_tokenizer_spells_every_ascii_byte() {
    // bpe already refuses a vocabulary whose `}` is not a plain token, so the
    // engine's own check is a second line that only a change to bpe could reach.
    let tokens: Vec<String> = (0..=255u8).map(|b| bpe::bytes::to_char(b).into()).collect();
    let types = (0..=255u8).map(|b| if b == b'}' { 5i64 } else { 1 });
    let tok = Tokenizer::new(
        tokens.iter().map(String::as_str),
        Some(types),
        [].into_iter(),
    );
    assert_eq!(tok.err(), Some(bpe::Error::MissingByte(b'}')));
    let tok = vocabulary(&[b"ab", b"\n\n", b"\xc3\xa9"]);
    let ascii = ascii_tokens(&tok).unwrap();
    assert!(spells_ascii(&tok, &ascii));
    assert_eq!(ascii.len(), 95 + 1);
    assert!(!spells_ascii(&tok, &ascii[1..]));
}

/// A text field, a choice and a list of choices, as M10's `tune` answer has.
const CHOSEN: [Field; 3] = [
    SMALL[0],
    Field {
        name: "p",
        max_chars: 1,
        max_lines: 1,
        max_line_chars: 1,
        trim_start: false,
        hard: false,
    },
    Field {
        name: "bg",
        max_chars: 1,
        max_lines: 1,
        max_line_chars: 1,
        trim_start: false,
        hard: false,
    },
];
const WORDS: [&str; 3] = ["ab", "abc", "x"];
const NAMES: [&str; 2] = ["u", "vw"];
const CHOICES: [crate::json::Choice<'static>; 2] = [
    crate::json::Choice {
        field: 1,
        words: &WORDS,
        list: None,
    },
    crate::json::Choice {
        field: 2,
        words: &NAMES,
        list: Some(2),
    },
];

fn takes(grammar: &Grammar<'_>, text: &str) -> Result<State, Stop> {
    grammar.advance(grammar.start(), text.as_bytes())
}

#[test]
fn choices_take_only_their_words_and_lists_only_their_length() {
    let shape = Shape {
        fields: &CHOSEN,
        choices: &CHOICES,
    };
    let grammar = Grammar::new(&shape).unwrap();
    for whole in [
        r#"{"a":"hi","p":"abc","bg":["u","vw"]}"#,
        r#"{"a":"","p":"ab","bg":[]}"#,
        r#"{"a":"","p":"x","bg":["vw"]}"#,
    ] {
        assert!(takes(&grammar, whole).unwrap().is_done(), "{whole}");
    }
    for refused in [
        r#"{"a":"","p":"a""#,
        r#"{"a":"","p":"abcd"#,
        r#"{"a":"","p":"y"#,
        r#"{"a":"","p":"x","bg":["u","vw","#,
        r#"{"a":"","p":"x","bg":["u",]"#,
        r#"{"a":"","p":"x","bg":["v"]"#,
        r#"{"a":"","p":"x","bg":[,"#,
        r#"{"a":"","p":"x","bg":"u"#,
        r#"{"a":"","p":"x","bg":["u"]"}"#,
        r#"{"a":"","p":"\"#,
    ] {
        assert_eq!(
            takes(&grammar, refused).err(),
            Some(Stop::Masked),
            "{refused}"
        );
    }
    // A list takes a word once: with both taken, only the end may come.
    let repeat = r#"{"a":"","p":"x","bg":["u","u"#;
    assert_eq!(takes(&grammar, repeat).err(), Some(Stop::Masked));
    let mut more = CHOICES[1];
    more.list = Some(5);
    let roomy = [CHOICES[0], more];
    let grammar = Grammar::new(&Shape {
        fields: &CHOSEN,
        choices: &roomy,
    })
    .unwrap();
    let both = r#"{"a":"","p":"x","bg":["vw","u","#;
    assert_eq!(takes(&grammar, both).err(), Some(Stop::Masked));
}

#[test]
fn need_is_the_true_distance_with_choices() {
    let only_list = [CHOICES[1]];
    let first = [crate::json::Choice {
        field: 0,
        ..CHOICES[0]
    }];
    for (fields, choices) in [
        (&CHOSEN[..], &CHOICES[..]),
        (&CHOSEN[..], &only_list[..]),
        (&CHOSEN[1..2], &first[..]),
    ] {
        let grammar = Grammar::new(&Shape { fields, choices }).unwrap();
        let states = distances(&grammar);
        for (state, distance) in &states {
            assert_eq!(Some(grammar.need(state)), *distance, "{state:?}");
        }
    }
}

#[test]
fn choices_are_checked() {
    let bad = |choice: crate::json::Choice<'static>| {
        let choices = [choice];
        Grammar::new(&Shape {
            fields: &CHOSEN,
            choices: &choices,
        })
        .err()
    };
    let wrong = Some(Error::Shape(
        "has a choice whose words are not plain and distinct",
    ));
    assert_eq!(
        bad(crate::json::Choice {
            words: &["a", "a"],
            ..CHOICES[0]
        }),
        wrong
    );
    assert_eq!(
        bad(crate::json::Choice {
            words: &["a\"b"],
            ..CHOICES[0]
        }),
        wrong
    );
    assert_eq!(
        bad(crate::json::Choice {
            words: &[""],
            ..CHOICES[0]
        }),
        wrong
    );
    assert_eq!(
        bad(crate::json::Choice {
            words: &[],
            ..CHOICES[0]
        }),
        wrong
    );
    // An empty list is fine: the answer can only be `[]`.
    assert!(
        bad(crate::json::Choice {
            words: &[],
            ..CHOICES[1]
        })
        .is_none()
    );
    assert_eq!(
        bad(crate::json::Choice {
            field: 3,
            ..CHOICES[0]
        }),
        Some(Error::Shape("has a choice for no field, or two for one"))
    );
    let twice = [CHOICES[0], CHOICES[0]];
    let shape = Shape {
        fields: &CHOSEN,
        choices: &twice,
    };
    assert!(Grammar::new(&shape).is_err());
}

#[test]
fn a_hard_limit_masks_what_a_soft_one_lets_end_the_answer() {
    let hard = [
        Field {
            hard: true,
            ..SMALL[0]
        },
        SMALL[1],
    ];
    let grammar = Grammar::new(&Shape {
        fields: &hard,
        choices: &[],
    })
    .unwrap();
    // "a" takes 3 characters a line: a fourth is masked, not an overflow.
    let at = grammar.advance(grammar.start(), br#"{"a":"xyz"#).unwrap();
    assert_eq!(grammar.step(at, b'w'), Err(Stop::Masked));
    assert!(grammar.step(at, b'"').is_ok());
    let soft = Grammar::new(&Shape {
        fields: &SMALL,
        choices: &[],
    })
    .unwrap();
    let at = soft.advance(soft.start(), br#"{"a":"xyz"#).unwrap();
    assert!(matches!(soft.step(at, b'w'), Err(Stop::Over(_))));
    // Every state the hard shape takes can still close, at the distance `need` says.
    for (state, distance) in &distances(&grammar) {
        assert_eq!(Some(grammar.need(state)), *distance, "{state:?}");
    }
}
