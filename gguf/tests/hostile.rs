//! Hostile files: every prefix, every field set to an extreme value and thousands
//! of random mutations of a valid file either parse or fail with an error. None
//! panics, and none makes the reader allocate more than a small multiple of the
//! file's size.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use gguf::write::Writer;
use gguf::{Error, Gguf, Header, TensorType, ValueType};

/// Bytes the reader may allocate per byte of input, plus a floor: the maps cost
/// tens of bytes per entry, and the smallest entry takes 13 bytes of file.
const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 4096;

/// Every value type, string and integer arrays, a set alignment and three tensor
/// types, so every branch of the reader has bytes to mutate.
fn sample() -> Vec<u8> {
    let mut w = Writer::new();
    w.alignment(64)
        .string("general.architecture", "qwen3")
        .u32("qwen3.block_count", 2)
        .f32("qwen3.rope.freq_base", 1e6)
        .bool("tokenizer.ggml.add_bos_token", false)
        .raw("u8", ValueType::U8.id(), &[7])
        .raw("i16", ValueType::I16.id(), &(-3i16).to_le_bytes())
        .raw("u64", ValueType::U64.id(), &u64::MAX.to_le_bytes())
        .raw("f64", ValueType::F64.id(), &1.5f64.to_le_bytes())
        .strings("tokenizer.ggml.tokens", &["a", "b", "ab", "\u{120}x"])
        .i32s("tokenizer.ggml.token_type", &[1, 1, 1, 3])
        .tensor("norm", &[4], TensorType::F32, &[1; 16])
        .tensor("w", &[32, 2], TensorType::Q8_0, &[7; 68])
        .tensor("h", &[3, 1, 1], TensorType::F16, &[9; 6]);
    w.finish()
}

/// Parses `bytes` as both a header and a whole file and reads everything a
/// caller can: every metadata value, every array element, every tensor's data.
fn exercise(bytes: &[u8]) -> Result<(), Error> {
    let header = Header::parse(bytes)?;
    for (_, value) in header.metadata() {
        if let Some(array) = value.as_array() {
            if let Some(strs) = array.strs() {
                // An array is walked when it is read, so it yields every element.
                assert_eq!(strs.count(), array.len());
            }
            if let Some(ints) = array.ints() {
                assert_eq!(ints.count(), array.len());
            }
        }
    }
    assert!(header.data_offset() >= header.table_end());
    assert_eq!(header.data_offset() % header.alignment(), 0);
    let file = Gguf::parse(bytes)?;
    for info in file.header().tensors() {
        let tensor = file.tensor(info.name).expect("a checked tensor has data");
        assert_eq!(tensor.data.len() as u64, info.size);
        assert_eq!(info.elements(), info.dims().iter().product::<u64>());
    }
    Ok(())
}

fn check(bytes: &[u8]) -> Result<(), Error> {
    let (result, peak) = hostile::peak_alloc(|| exercise(bytes));
    let cap = ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR;
    assert!(
        peak <= cap,
        "{peak} bytes allocated for {} bytes",
        bytes.len()
    );
    result
}

#[test]
fn the_sample_parses() {
    check(&sample()).unwrap();
    // The counter sees the maps, so the caps below mean something.
    let (_, peak) = hostile::peak_alloc(|| Header::parse(&sample()).map(|_| ()));
    assert!(peak > 0);
}

#[test]
fn every_prefix_fails_short_until_the_table_ends() {
    let bytes = sample();
    let table_end = Header::parse(&bytes).unwrap().table_end() as usize;
    hostile::truncations(&bytes, |cut, prefix| {
        let header = Header::parse(prefix);
        if cut < table_end {
            let error = header.unwrap_err();
            assert!(error.is_short(), "cut at {cut}: {error:?}");
        } else {
            assert!(header.is_ok(), "cut at {cut}");
        }
        let whole = check(prefix);
        assert_eq!(whole.is_ok(), cut == bytes.len(), "cut at {cut}");
    });
}

#[test]
fn every_field_set_to_every_extreme() {
    let bytes = sample();
    let table_end = Header::parse(&bytes).unwrap().table_end() as usize;
    hostile::sweep_fields(&bytes, 0..table_end, |_, mutated| {
        let _ = check(mutated);
    });
}

#[test]
fn random_mutations() {
    let bytes = sample();
    for seed in [1, 0x9e37_79b9_7f4a_7c15, 0xdead_beef] {
        hostile::fuzz(&bytes, seed, 4000, |mutated| {
            let _ = check(mutated);
        });
    }
}

/// The header fields by hand, for files the writer would not lay out.
fn raw_file(tensor_count: u64, kv_count: u64, rest: &[u8]) -> Vec<u8> {
    let mut out = b"GGUF".to_vec();
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&tensor_count.to_le_bytes());
    out.extend_from_slice(&kv_count.to_le_bytes());
    out.extend_from_slice(rest);
    out
}

#[test]
fn huge_counts_fail_before_allocating() {
    for (tensors, kvs) in [
        (gguf::MAX_TENSORS, 0),
        (0, gguf::MAX_METADATA),
        (gguf::MAX_TENSORS, gguf::MAX_METADATA),
        (u64::MAX, 0),
        (0, u64::MAX),
    ] {
        let bytes = raw_file(tensors, kvs, &[0; 64]);
        let (result, peak) = hostile::peak_alloc(|| Header::parse(&bytes).map(|_| ()));
        assert!(result.is_err());
        assert!(
            peak <= ALLOC_FLOOR,
            "{peak} bytes for counts {tensors}, {kvs}"
        );
    }
    // A string array that claims u64::MAX empty strings: each takes 8 bytes, so
    // it runs out of bytes instead of looping.
    let mut payload = ValueType::Str.id().to_le_bytes().to_vec();
    payload.extend_from_slice(&u64::MAX.to_le_bytes());
    payload.extend_from_slice(&[0; 8 * 1000]);
    let mut w = Writer::new();
    w.raw("k", ValueType::Array.id(), &payload);
    assert_eq!(check(&w.finish()), Err(Error::Truncated));
}

#[test]
fn extreme_shapes_and_alignments() {
    // Dims whose product overflows, a zero dimension, and the widest shape.
    for (dims, expected) in [
        (
            &[1u64 << 16, 1 << 16, 1 << 16, 1 << 16][..],
            Err(Error::DimsOverflow),
        ),
        (&[u64::MAX, u64::MAX], Err(Error::DimsOverflow)),
        (&[0, 7], Ok(())),
        (&[1, 1, 1, 1], Ok(())),
    ] {
        let mut w = Writer::new();
        w.tensor("w", dims, TensorType::F32, &[0; 4]);
        assert_eq!(check(&w.finish()), expected, "{dims:?}");
    }
    // The largest power-of-two alignment: the data offset still fits.
    let mut w = Writer::new();
    w.u32(gguf::ALIGNMENT_KEY, 1 << 31)
        .raw_tensor("w", &[1], TensorType::F32.id(), 0);
    assert_eq!(check(&w.finish()), Err(Error::TensorPastEnd));
    // Zero, one and the largest u32 are not alignments.
    for alignment in [0, 1, u32::MAX] {
        let mut w = Writer::new();
        w.u32(gguf::ALIGNMENT_KEY, alignment);
        assert_eq!(check(&w.finish()), Err(Error::BadAlignment));
    }
    // An offset that is aligned but runs past u64 once the data start is added.
    let mut w = Writer::new();
    w.raw_tensor("w", &[8], TensorType::F32.id(), !31);
    assert_eq!(check(&w.finish()), Err(Error::TensorPastEnd));
}
