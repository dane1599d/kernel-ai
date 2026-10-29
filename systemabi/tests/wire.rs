//! The sample's bytes: the layout the docs give, every way bytes can fail to be
//! a sample, and mutated samples, which must never panic.

use systemabi::{
    Error, MAGIC, MAX_BYTES, MAX_NAME, MAX_PROCS, ModelState, ModelStatus, ProcSample, Sample,
};

fn proc(pid: u32, name: &str, pages: u64, cpu_ns: u64) -> ProcSample {
    ProcSample {
        pid,
        name: name.into(),
        pages,
        cpu_ns,
    }
}

fn usual() -> Sample {
    Sample {
        uptime_ns: 754_000_000_000,
        cpus: 4,
        free_pages: 384_000,
        total_pages: 1_048_576,
        heap_bytes: 839_680,
        procs: vec![
            proc(1, "supervisor", 75, 5_000_000),
            proc(4, "llm", 600_000, 612_000_000),
            proc(9, "shell", 40, 0),
        ],
        omitted: 2,
        model: ModelStatus {
            state: ModelState::Ready,
            tps10: 125,
        },
    }
}

/// The layout in the docs, written out by hand.
fn by_hand() -> Vec<u8> {
    let mut b = b"KSY1".to_vec();
    b.extend(754_000_000_000u64.to_le_bytes());
    b.extend(4u32.to_le_bytes());
    b.extend(384_000u64.to_le_bytes());
    b.extend(1_048_576u64.to_le_bytes());
    b.extend(839_680u64.to_le_bytes());
    b.extend(2u32.to_le_bytes());
    b.push(3);
    b.extend(125u32.to_le_bytes());
    b.extend(3u16.to_le_bytes());
    for (pid, name, pages, cpu) in [
        (1u32, "supervisor", 75u64, 5_000_000u64),
        (4, "llm", 600_000, 612_000_000),
        (9, "shell", 40, 0),
    ] {
        b.extend(pid.to_le_bytes());
        b.extend(pages.to_le_bytes());
        b.extend(cpu.to_le_bytes());
        b.push(name.len() as u8);
        b.extend(name.as_bytes());
    }
    b
}

#[test]
fn a_sample_is_the_documented_bytes_and_comes_back() {
    let sample = usual();
    assert_eq!(sample.encode(), by_hand());
    assert_eq!(Sample::parse(&by_hand()), Ok(sample));
}

#[test]
fn an_empty_machine_round_trips() {
    let sample = Sample {
        uptime_ns: 0,
        cpus: 0,
        free_pages: 0,
        total_pages: 0,
        heap_bytes: 0,
        procs: vec![],
        omitted: 0,
        model: ModelStatus::UNKNOWN,
    };
    assert_eq!(Sample::parse(&sample.encode()), Ok(sample));
}

#[test]
fn extreme_numbers_survive_the_trip() {
    let mut sample = usual();
    sample.uptime_ns = u64::MAX;
    sample.cpus = u32::MAX;
    sample.free_pages = u64::MAX;
    sample.total_pages = u64::MAX;
    sample.omitted = u32::MAX;
    sample.model.tps10 = u32::MAX;
    sample.procs = vec![proc(u32::MAX, "x", u64::MAX, u64::MAX)];
    assert_eq!(Sample::parse(&sample.encode()), Ok(sample));
}

#[test]
fn every_model_state_has_its_own_byte_and_unknown_bytes_are_refused() {
    let states = [
        ModelState::Unknown,
        ModelState::Waiting,
        ModelState::Warming,
        ModelState::Ready,
        ModelState::Remote,
        ModelState::Failed,
        ModelState::NoModel,
        ModelState::Busy,
    ];
    for (i, state) in states.into_iter().enumerate() {
        assert_eq!(usize::from(state.code()), i);
        assert_eq!(ModelState::from_code(state.code()), Some(state));
    }
    assert_eq!(ModelState::from_code(8), None);
    assert_eq!(ModelState::from_code(255), None);

    let mut bytes = by_hand();
    bytes[4 + 8 + 4 + 8 + 8 + 8 + 4] = 9;
    assert_eq!(Sample::parse(&bytes), Err(Error::BadModelState(9)));
}

#[test]
fn bytes_cut_anywhere_are_not_a_sample() {
    let bytes = by_hand();
    for len in 0..bytes.len() {
        let got = Sample::parse(&bytes[..len]);
        assert!(
            matches!(got, Err(Error::Truncated | Error::BadMagic)),
            "{len} bytes gave {got:?}"
        );
    }
}

#[test]
fn a_byte_too_many_and_a_wrong_magic_are_refused() {
    let mut bytes = by_hand();
    bytes.push(0);
    assert_eq!(Sample::parse(&bytes), Err(Error::Trailing));

    let mut bytes = by_hand();
    bytes[3] = b'2';
    assert_eq!(Sample::parse(&bytes), Err(Error::BadMagic));
    assert_eq!(MAGIC, *b"KSY1");
}

#[test]
fn a_process_count_the_bytes_cannot_hold_is_refused_before_anything_is_reserved() {
    let mut bytes = by_hand();
    let at = 4 + 8 + 4 + 8 + 8 + 8 + 4 + 1 + 4;
    bytes[at..at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(Sample::parse(&bytes), Err(Error::TooManyProcs(u16::MAX)));
    // A count within the limit that the bytes do not cover is cut short.
    bytes[at..at + 2].copy_from_slice(&200u16.to_le_bytes());
    assert_eq!(Sample::parse(&bytes), Err(Error::Truncated));
}

#[test]
fn names_that_are_too_long_or_not_text_are_refused() {
    let mut sample = usual();
    sample.procs = vec![proc(1, "ab", 1, 1)];
    let mut bytes = sample.encode();
    let len_at = bytes.len() - 3;
    assert_eq!(bytes[len_at], 2);

    let mut long = bytes.clone();
    long[len_at] = (MAX_NAME + 1) as u8;
    long.extend(vec![b'a'; MAX_NAME - 1]);
    assert_eq!(
        Sample::parse(&long),
        Err(Error::NameLength((MAX_NAME + 1) as u8))
    );

    bytes[len_at + 1] = 0xff;
    assert_eq!(Sample::parse(&bytes), Err(Error::NameNotUtf8));
}

#[test]
fn a_sample_over_the_limit_is_refused_unread() {
    let mut bytes = by_hand();
    bytes.resize(MAX_BYTES + 1, 0);
    assert_eq!(Sample::parse(&bytes), Err(Error::TooLarge));
}

#[test]
fn encoding_cuts_what_would_not_parse_and_says_so() {
    let mut sample = usual();
    // 63 bytes of ASCII and then a 2-byte character that would straddle the limit.
    let name = format!("{}é{}", "n".repeat(MAX_NAME - 1), "tail");
    sample.procs = (0..MAX_PROCS as u32 + 40)
        .map(|i| proc(i, &name, 1, 1))
        .collect();
    sample.omitted = 5;
    let bytes = sample.encode();
    assert!(bytes.len() <= MAX_BYTES);
    let got = Sample::parse(&bytes).expect("what is written parses");
    assert_eq!(got.procs.len(), MAX_PROCS);
    assert_eq!(got.omitted, 45);
    assert!(got.procs.iter().all(|p| p.name == "n".repeat(MAX_NAME - 1)));
}

#[test]
fn mutated_samples_never_panic_and_what_parses_round_trips() {
    // A fixed xorshift, so a failure is the same failure next time.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let seed = by_hand();
    let mut parsed = 0;
    for _ in 0..20_000 {
        let mut bytes = seed.clone();
        for _ in 0..1 + next() % 4 {
            let at = (next() % bytes.len() as u64) as usize;
            bytes[at] = next() as u8;
        }
        if next() % 5 == 0 {
            bytes.truncate((next() % bytes.len() as u64) as usize);
        }
        if let Ok(sample) = Sample::parse(&bytes) {
            parsed += 1;
            assert_eq!(Sample::parse(&sample.encode()), Ok(sample));
        }
    }
    assert!(parsed > 1000, "only {parsed} mutated samples still parsed");
}
