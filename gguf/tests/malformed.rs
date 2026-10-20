//! Every malformed file fails with its named error and never panics.

use gguf::write::Writer;
use gguf::{Error, Gguf, Header, TensorType, ValueType};

/// A file with one Q8_0 tensor, for cutting and editing.
fn valid() -> Writer {
    let mut w = Writer::new();
    w.string("general.architecture", "qwen3")
        .tensor("w", &[32, 2], TensorType::Q8_0, &[1; 68]);
    w
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
fn truncated_header() {
    let bytes = valid().finish();
    assert_eq!(Gguf::parse(&bytes[..0]).unwrap_err(), Error::Truncated);
    assert_eq!(Gguf::parse(&bytes[..3]).unwrap_err(), Error::Truncated);
    assert_eq!(Gguf::parse(&bytes[..10]).unwrap_err(), Error::Truncated);
    assert_eq!(Gguf::parse(&bytes[..23]).unwrap_err(), Error::Truncated);
    // Inside the first key's length field.
    assert_eq!(Gguf::parse(&bytes[..28]).unwrap_err(), Error::Truncated);
    // A count that promises more entries than the bytes hold.
    let bytes = raw_file(0, 1000, &[]);
    assert_eq!(Gguf::parse(&bytes).unwrap_err(), Error::Truncated);
}

#[test]
fn huge_tensor_count_is_refused_before_allocating() {
    let bytes = raw_file(gguf::MAX_TENSORS, 0, &[0; 64]);
    assert_eq!(Header::parse(&bytes).unwrap_err(), Error::Truncated);
}

#[test]
fn string_past_eof() {
    // A key whose length is more than the bytes left.
    let mut rest = 100u64.to_le_bytes().to_vec();
    rest.extend_from_slice(b"short");
    let bytes = raw_file(0, 1, &rest);
    assert_eq!(Gguf::parse(&bytes).unwrap_err(), Error::StringPastEnd);
    // A length that does not even fit in usize arithmetic.
    let bytes = raw_file(0, 1, &u64::MAX.to_le_bytes());
    assert_eq!(Gguf::parse(&bytes).unwrap_err(), Error::StringPastEnd);
    // A string value, and a string inside an array.
    let mut w = Writer::new();
    w.raw("k", ValueType::Str.id(), &(1u64 << 63).to_le_bytes());
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::StringPastEnd);
    let mut payload = ValueType::Str.id().to_le_bytes().to_vec();
    payload.extend_from_slice(&2u64.to_le_bytes());
    payload.extend_from_slice(&1u64.to_le_bytes());
    payload.push(b'a');
    // Longer than the rest of the file, padding included.
    payload.extend_from_slice(&1000u64.to_le_bytes());
    let mut w = Writer::new();
    w.raw("k", ValueType::Array.id(), &payload);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::StringPastEnd);
}

#[test]
fn array_longer_than_the_file() {
    let mut payload = ValueType::U32.id().to_le_bytes().to_vec();
    payload.extend_from_slice(&(u64::MAX / 2).to_le_bytes());
    let mut w = Writer::new();
    w.raw("k", ValueType::Array.id(), &payload);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::Truncated);
    // A string array with a huge count runs out of bytes, it does not loop forever.
    let mut payload = ValueType::Str.id().to_le_bytes().to_vec();
    payload.extend_from_slice(&u64::MAX.to_le_bytes());
    let mut w = Writer::new();
    w.raw("k", ValueType::Array.id(), &payload);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::Truncated);
}

#[test]
fn tensor_past_eof() {
    let bytes = valid().finish();
    // The last data byte is missing.
    assert_eq!(
        Gguf::parse(&bytes[..bytes.len() - 1]).unwrap_err(),
        Error::TensorPastEnd
    );
    // An offset beyond the file, and one that overflows when the data start is added.
    for offset in [1 << 20, u64::MAX - 31] {
        let mut w = Writer::new();
        w.raw_tensor("w", &[32], TensorType::Q8_0.id(), offset);
        let bytes = w.finish();
        assert!(Header::parse(&bytes).is_ok());
        assert_eq!(Gguf::parse(&bytes).unwrap_err(), Error::TensorPastEnd);
    }
    // The start fits, but the start plus the size does not.
    let mut w = Writer::new();
    w.raw_tensor("w", &[32], TensorType::Q8_0.id(), u64::MAX - 95);
    let bytes = w.finish();
    let header = Header::parse(&bytes).unwrap();
    assert!(header.data_offset().checked_add(u64::MAX - 95).is_some());
    assert_eq!(Gguf::parse(&bytes).unwrap_err(), Error::TensorPastEnd);
}

#[test]
fn too_many_entries() {
    let bytes = raw_file(0, gguf::MAX_METADATA + 1, &[0; 64]);
    assert_eq!(Gguf::parse(&bytes).unwrap_err(), Error::TooMany);
    let bytes = raw_file(gguf::MAX_TENSORS + 1, 0, &[0; 64]);
    assert_eq!(Gguf::parse(&bytes).unwrap_err(), Error::TooMany);
}

#[test]
fn overflowing_dims() {
    for dims in [&[u64::MAX, 2][..], &[1 << 32, 1 << 32], &[1 << 62, 1]] {
        let mut w = Writer::new();
        w.raw_tensor("w", dims, TensorType::F32.id(), 0);
        assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::DimsOverflow);
    }
    // Q8_0 rows of a huge length overflow in bytes, not elements.
    let mut w = Writer::new();
    w.raw_tensor("w", &[u64::MAX - 31], TensorType::Q8_0.id(), 0);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::DimsOverflow);
}

#[test]
fn bad_dim_counts() {
    for dims in [&[][..], &[1, 1, 1, 1, 1]] {
        let mut w = Writer::new();
        w.raw_tensor("w", dims, TensorType::F32.id(), 0);
        let expected = Error::BadDimCount(dims.len() as u32);
        assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), expected);
    }
}

#[test]
fn partial_block() {
    let mut w = Writer::new();
    w.raw_tensor("w", &[48, 2], TensorType::Q8_0.id(), 0);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::PartialBlock);
    // A K-quant row of 255, 300 or 32 values is not whole super-blocks.
    for ty in [TensorType::Q4_K, TensorType::Q6_K] {
        for row in [255, 300, 32] {
            let mut w = Writer::new();
            w.raw_tensor("w", &[row, 2], ty.id(), 0);
            assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::PartialBlock);
        }
    }
}

#[test]
fn k_quant_tensors_inside_and_past_the_end() {
    // Whole super-blocks parse, and their data is the size rule's bytes; one
    // byte short of that is past the end.
    for (ty, bytes) in [(TensorType::Q4_K, 2 * 144), (TensorType::Q6_K, 2 * 210)] {
        let mut w = Writer::new();
        w.tensor("w", &[256, 2], ty, &vec![7; bytes]);
        let file = w.finish();
        let parsed = Gguf::parse(&file).unwrap();
        let tensor = parsed.tensor("w").unwrap();
        assert_eq!((tensor.info.ty, tensor.data.len()), (ty, bytes));
        let cut = &file[..file.len() - 1];
        assert_eq!(Gguf::parse(cut).unwrap_err(), Error::TensorPastEnd);
    }
    // A K row so long its bytes overflow nothing but the file: refused as past
    // the end, through the checked start plus size.
    let mut w = Writer::new();
    w.raw_tensor("w", &[1 << 63], TensorType::Q4_K.id(), 0);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::TensorPastEnd);
    let mut w = Writer::new();
    w.raw_tensor("w", &[256, 1 << 56], TensorType::Q6_K.id(), 0);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::DimsOverflow);
}

#[test]
fn bad_alignment() {
    // An offset that is not a multiple of the alignment.
    let mut w = Writer::new();
    w.raw_tensor("w", &[1], TensorType::F32.id(), 4);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::BadAlignment);
    // Alignments that are not powers of two, and one of the wrong type.
    for alignment in [0u32, 1, 2, 3, 4, 48] {
        let mut w = Writer::new();
        w.u32(gguf::ALIGNMENT_KEY, alignment);
        assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::BadAlignment);
    }
    let mut w = Writer::new();
    w.string(gguf::ALIGNMENT_KEY, "32");
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::BadAlignment);
    let mut w = Writer::new();
    w.raw(
        gguf::ALIGNMENT_KEY,
        ValueType::U64.id(),
        &32u64.to_le_bytes(),
    );
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::BadAlignment);
}

#[test]
fn unknown_type() {
    // Q4_0, Q5_K, BF16, nothing, and the top id: Q4_K (12) and Q6_K (14) parse.
    for id in [2, 13, 30, 99, u32::MAX] {
        let mut w = Writer::new();
        w.raw_tensor("w", &[256], id, 0);
        assert_eq!(
            Gguf::parse(&w.finish()).unwrap_err(),
            Error::UnknownType(id)
        );
    }
    let mut w = Writer::new();
    w.raw("k", 13, &[0; 8]);
    assert_eq!(
        Gguf::parse(&w.finish()).unwrap_err(),
        Error::UnknownValueType(13)
    );
}

#[test]
fn bad_magic_and_version() {
    let mut bytes = valid().finish();
    bytes[0] = b'g';
    assert_eq!(Gguf::parse(&bytes).unwrap_err(), Error::BadMagic);
    let mut bytes = valid().finish();
    bytes[4] = 2;
    assert_eq!(
        Gguf::parse(&bytes).unwrap_err(),
        Error::UnsupportedVersion(2)
    );
}

#[test]
fn nested_arrays_and_bad_utf8() {
    let mut payload = ValueType::Array.id().to_le_bytes().to_vec();
    payload.extend_from_slice(&0u64.to_le_bytes());
    let mut w = Writer::new();
    w.raw("k", ValueType::Array.id(), &payload);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::NestedArray);
    let mut payload = 2u64.to_le_bytes().to_vec();
    payload.extend_from_slice(&[0xc3, 0x28]);
    let mut w = Writer::new();
    w.raw("k", ValueType::Str.id(), &payload);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::BadUtf8);
}

#[test]
fn duplicates() {
    let mut w = Writer::new();
    w.u32("k", 1).u32("k", 2);
    assert_eq!(Gguf::parse(&w.finish()).unwrap_err(), Error::DuplicateKey);
    let mut w = Writer::new();
    w.tensor("w", &[1], TensorType::F32, &[0; 4])
        .tensor("w", &[1], TensorType::F32, &[0; 4]);
    assert_eq!(
        Gguf::parse(&w.finish()).unwrap_err(),
        Error::DuplicateTensor
    );
}
