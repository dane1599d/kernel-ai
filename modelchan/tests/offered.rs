//! `kai-offers 1` (P5 task 9): what `updated` answers `models offers` with,
//! written from `fetch::offers` and read back by the shell, and refused whole
//! when a line is not one.

use comp_models::{Signed, Store};
use modelchan::fetch::{self, Fit, HUGGING_FACE, Machine};
use modelchan::offered::{MAGIC, MAX_LINES, Offered, encode, parse};

const GIB: u64 = 1 << 30;

fn signed(id: &str, length: u64, sha: u8) -> Signed {
    Signed {
        id: id.into(),
        name: format!("{id} name"),
        length,
        sha256: [sha; 32],
        licence: "Apache-2.0".into(),
        source: format!(
            "https://huggingface.co/o/r/resolve/{}/{id}.gguf",
            "a".repeat(40)
        ),
    }
}

static EMPTY: Store = Store {
    blobs: Vec::new(),
    running: None,
    next: None,
    on_trial: false,
    rolled_back: None,
};

fn machine(memory_mib: u64, free_bytes: u64) -> Machine<'static> {
    Machine {
        memory_mib,
        free_bytes,
        store: &EMPTY,
        partials: &[],
        hosts: &[HUGGING_FACE],
    }
}

#[test]
fn what_offers_gives_reads_back_the_same() {
    let role = [
        signed("small", 600_000_000, 1),
        signed("mid", 1_100_000_000, 2),
        signed("big", 2_500_000_000, 3),
    ];
    let offers = fetch::offers(&role, &machine(4096, 8 * GIB));
    let text = encode(&offers);
    assert!(text.starts_with(MAGIC));
    let back = parse(&text).unwrap();
    let want: Vec<Offered> = offers
        .iter()
        .map(|o| Offered {
            id: o.target.id.clone(),
            fit: o.fit,
            recommended: o.recommended,
        })
        .collect();
    assert_eq!(back, want);
    assert_eq!(back[0].id, "big");
    assert!(matches!(back[0].fit, Fit::Memory { .. }));
    assert_eq!(back[1].id, "mid");
    assert!(back[1].recommended);
    // Short of disk, with the numbers kept.
    let tight = fetch::offers(&role, &machine(8192, GIB));
    let back = parse(&encode(&tight)).unwrap();
    assert!(back.iter().all(|o| !o.recommended));
    assert!(matches!(back[0].fit, Fit::Disk { free, .. } if free == GIB));
    // No targets: the magic alone.
    assert_eq!(parse(&encode(&[])).unwrap(), []);
}

#[test]
fn the_answer_parses_and_fits_one_message_whatever_the_role() {
    // Ids a line cannot carry are left out, and no more than MAX_LINES.
    let mut role: Vec<Signed> = (0..100)
        .map(|i| signed(&format!("{i:0>64}"), u64::MAX - i, i as u8))
        .collect();
    role.push(signed(&"y".repeat(65), 1, 0xfe));
    role.push(signed("has space", 1, 0xff));
    let offers = fetch::offers(&role, &machine(u64::MAX, u64::MAX));
    let text = encode(&offers);
    assert!(text.len() <= kabi::MAX_MESSAGE, "{}", text.len());
    let back = parse(&text).unwrap();
    assert_eq!(back.len(), MAX_LINES);
    assert!(back.iter().all(|o| o.id.len() == 64));
}

#[test]
fn a_line_that_is_not_one_refuses_the_whole_answer() {
    let bad = [
        "",
        "kai-offers 2\n",
        "kai-offers 1\nmid 1 fits 0\n",
        "kai-offers 1\nmid 1 fits 0 0 extra\n",
        "kai-offers 1\nmid 2 fits 0 0\n",
        // Recommended, but it does not fit.
        "kai-offers 1\nmid 1 memory 4160 4096\n",
        "kai-offers 1\nmid 0 stored 1 0\n",
        "kai-offers 1\nmid 0 roomy 0 0\n",
        "kai-offers 1\nmid 0 disk -1 0\n",
        "kai-offers 1\nmid 0 disk 18446744073709551616 0\n",
        "kai-offers 1\n 0 fits 0 0\n",
        "kai-offers 1\nmi\u{7f}d 0 fits 0 0\n",
        "kai-offers 1\na 1 fits 0 0\nb 1 fits 0 0\n",
    ];
    for text in bad {
        assert!(parse(text).is_err(), "{text:?}");
    }
    let long = format!("kai-offers 1\n{} 0 fits 0 0\n", "x".repeat(65));
    assert!(parse(&long).is_err());
    let many: String = (0..65).map(|i| format!("m{i} 0 fits 0 0\n")).collect();
    assert!(parse(&format!("kai-offers 1\n{many}")).is_err());
}

#[test]
fn hostile_answers_never_panic() {
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let alphabet = b"kai-offers 1\n0123456789 fitsdkmryo\x00\xff";
    for _ in 0..2_000 {
        let len = (x % 80) as usize;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                alphabet[(x % alphabet.len() as u64) as usize]
            })
            .collect();
        let text = String::from_utf8_lossy(&bytes);
        let _ = parse(&format!("{MAGIC}\n{text}"));
        let _ = parse(&text);
    }
}
