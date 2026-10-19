//! Round trips through the writer. Malformed files are in `tests/malformed.rs`.

use crate::write::Writer;
use crate::*;
use alloc::vec::Vec;

fn sample() -> Vec<u8> {
    let mut w = Writer::new();
    w.string("general.architecture", "qwen3")
        .u32("qwen3.block_count", 2)
        .f32("qwen3.rope.freq_base", 1e6)
        .bool("tokenizer.ggml.add_bos_token", false)
        .strings("tokenizer.ggml.tokens", &["a", "b", "ab", "\u{120}x"])
        .i32s("tokenizer.ggml.token_type", &[1, 1, 1, 3])
        .tensor("norm", &[4], TensorType::F32, &[1; 16])
        .tensor("w", &[32, 2], TensorType::Q8_0, &[7; 68])
        .tensor("h", &[3], TensorType::F16, &[9; 6]);
    w.finish()
}

#[test]
fn reads_back_what_the_writer_wrote() {
    let bytes = sample();
    let file = Gguf::parse(&bytes).unwrap();
    assert_eq!(
        file.get("general.architecture").unwrap().as_str(),
        Some("qwen3")
    );
    assert_eq!(file.get("qwen3.block_count").unwrap().as_u32(), Some(2));
    assert_eq!(
        file.get("qwen3.rope.freq_base").unwrap().as_f32(),
        Some(1e6)
    );
    assert_eq!(
        file.get("tokenizer.ggml.add_bos_token").unwrap().as_bool(),
        Some(false)
    );
    let tokens = file
        .get("tokenizer.ggml.tokens")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(tokens.len(), 4);
    assert_eq!(
        tokens.strs().unwrap().collect::<Vec<_>>(),
        ["a", "b", "ab", "\u{120}x"]
    );
    assert!(tokens.ints().is_none());
    let types = file
        .get("tokenizer.ggml.token_type")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(types.ints().unwrap().collect::<Vec<_>>(), [1, 1, 1, 3]);
    assert!(types.strs().is_none());

    let w = file.tensor("w").unwrap();
    assert_eq!(w.info.dims(), [32, 2]);
    assert_eq!(w.info.ty, TensorType::Q8_0);
    assert_eq!(w.info.elements(), 64);
    assert_eq!(w.data, [7; 68]);
    assert_eq!(file.tensor("norm").unwrap().data, [1; 16]);
    assert_eq!(file.tensor("h").unwrap().data, [9; 6]);
    assert!(file.tensor("missing").is_none());
    let header = file.header();
    assert_eq!(header.alignment(), DEFAULT_ALIGNMENT);
    assert_eq!(header.data_offset() % 32, 0);
    let names: Vec<_> = header.tensors().iter().map(|t| t.name).collect();
    assert_eq!(names, ["norm", "w", "h"]);
}

#[test]
fn integer_accessors_convert_only_when_the_value_fits() {
    let mut w = Writer::new();
    w.raw("neg", ValueType::I32.id(), &(-1i32).to_le_bytes())
        .raw("big", ValueType::U64.id(), &(u64::MAX).to_le_bytes())
        .raw("small", ValueType::U8.id(), &[200])
        .raw("wide", ValueType::F64.id(), &2.5f64.to_le_bytes());
    let bytes = w.finish();
    let h = Header::parse(&bytes).unwrap();
    assert_eq!(h.get("neg").unwrap().as_u64(), None);
    assert_eq!(h.get("big").unwrap().as_u64(), Some(u64::MAX));
    assert_eq!(h.get("big").unwrap().as_u32(), None);
    assert_eq!(h.get("small").unwrap().as_u32(), Some(200));
    assert_eq!(h.get("wide").unwrap().as_f32(), Some(2.5));
    assert_eq!(h.get("wide").unwrap().as_u32(), None);
    assert_eq!(h.metadata().count(), 4);
}

#[test]
fn integer_arrays_of_every_width_read_as_i64() {
    let mut w = Writer::new();
    let array = |ty: ValueType, items: &[&[u8]]| {
        let mut payload = ty.id().to_le_bytes().to_vec();
        payload.extend_from_slice(&(items.len() as u64).to_le_bytes());
        for item in items {
            payload.extend_from_slice(item);
        }
        payload
    };
    w.raw("u8", 9, &array(ValueType::U8, &[&[255]]))
        .raw("i16", 9, &array(ValueType::I16, &[&(-2i16).to_le_bytes()]))
        .raw(
            "u64",
            9,
            &array(
                ValueType::U64,
                &[&7u64.to_le_bytes(), &u64::MAX.to_le_bytes()],
            ),
        )
        .raw("i64", 9, &array(ValueType::I64, &[&(-5i64).to_le_bytes()]))
        .raw("f32", 9, &array(ValueType::F32, &[&1f32.to_le_bytes()]));
    let bytes = w.finish();
    let h = Header::parse(&bytes).unwrap();
    let ints = |key: &str| -> Vec<i64> {
        h.get(key)
            .unwrap()
            .as_array()
            .unwrap()
            .ints()
            .unwrap()
            .collect()
    };
    assert_eq!(ints("u8"), [255]);
    assert_eq!(ints("i16"), [-2]);
    assert_eq!(ints("u64"), [7, i64::MAX]);
    assert_eq!(ints("i64"), [-5]);
    assert!(h.get("f32").unwrap().as_array().unwrap().ints().is_none());
}

#[test]
fn custom_alignment_lays_out_and_checks_offsets() {
    let mut w = Writer::new();
    w.alignment(64)
        .tensor("a", &[1], TensorType::F32, &[1; 4])
        .tensor("b", &[1], TensorType::F32, &[2; 4]);
    let bytes = w.finish();
    let file = Gguf::parse(&bytes).unwrap();
    assert_eq!(file.header().alignment(), 64);
    assert_eq!(file.header().data_offset() % 64, 0);
    assert_eq!(file.header().tensor("b").unwrap().offset, 64);
    assert_eq!(file.tensor("b").unwrap().data, [2; 4]);
}

#[test]
fn header_alone_parses_without_the_data() {
    let bytes = sample();
    let header = Header::parse(&bytes).unwrap();
    let cut = &bytes[..header.data_offset() as usize];
    let header = Header::parse(cut).unwrap();
    assert_eq!(header.tensors().len(), 3);
    assert_eq!(
        header.check_extents(cut.len() as u64),
        Err(Error::TensorPastEnd)
    );
    assert_eq!(header.check_extents(bytes.len() as u64), Ok(()));
    assert_eq!(Gguf::parse(cut).unwrap_err(), Error::TensorPastEnd);
}

#[test]
fn size_rules() {
    assert_eq!(byte_size(TensorType::F32, &[3, 2]), Ok(24));
    assert_eq!(byte_size(TensorType::F16, &[3]), Ok(6));
    assert_eq!(byte_size(TensorType::Q8_0, &[64, 3]), Ok(6 * 34));
    assert_eq!(
        byte_size(TensorType::Q8_0, &[48, 2]),
        Err(Error::PartialBlock)
    );
    assert_eq!(
        byte_size(TensorType::F32, &[1 << 62, 1]),
        Err(Error::DimsOverflow)
    );
    assert_eq!(
        byte_size(TensorType::F32, &[1 << 32, 1 << 32]),
        Err(Error::DimsOverflow)
    );
    assert_eq!(byte_size(TensorType::F32, &[0, 7]), Ok(0));
}

#[test]
fn no_prefix_or_single_byte_change_breaks_the_reader() {
    let bytes = sample();
    // The last tensor ends at the end of the file, so no proper prefix is whole.
    for len in 0..bytes.len() {
        assert!(Gguf::parse(&bytes[..len]).is_err(), "{len}-byte prefix");
    }
    let header_len = Header::parse(&bytes).unwrap().data_offset() as usize;
    let mut copy = bytes.clone();
    for at in 0..header_len {
        for value in [0u8, 1, 0x7f, 0x80, 0xff] {
            copy[at] = value;
            // Whatever parses hands out every tensor at its declared size.
            if let Ok(file) = Gguf::parse(&copy) {
                for info in file.header().tensors() {
                    let tensor = file.tensor(info.name).unwrap();
                    assert_eq!(tensor.data.len() as u64, info.size);
                }
            }
        }
        copy[at] = bytes[at];
    }
}

#[test]
fn a_header_parses_exactly_from_the_end_of_its_table() {
    let bytes = sample();
    let header = Header::parse(&bytes).unwrap();
    let table_end = header.table_end() as usize;
    assert!(table_end <= header.data_offset() as usize);
    assert!(Header::parse(&bytes[..table_end]).is_ok());
    for len in 0..table_end {
        let error = Header::parse(&bytes[..len]).unwrap_err();
        assert!(error.is_short(), "{len}-byte prefix: {error:?}");
    }
}

#[test]
fn debug_output_never_holds_the_bytes() {
    let bytes = sample();
    let file = Gguf::parse(&bytes).unwrap();
    let text = alloc::format!("{file:?} {:?}", file.header());
    assert!(text.contains("Array(Str x 4)"), "{text}");
    assert!(!text.contains("[7, 7"), "{text}");
}

#[test]
fn errors_print_plainly() {
    let text = alloc::format!("{}", Error::UnknownType(12));
    assert_eq!(text, "tensor type 12 (Q4_K), which the engine cannot run");
    assert_eq!(type_name(30), "BF16");
    assert_eq!(type_name(39), "MXFP4");
    assert_eq!(type_name(99), "unknown");
}
