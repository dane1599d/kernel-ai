//! The body of a `gguf_file` fuzz target (plan P6 G32), which `hostile.rs`
//! runs over hostile files: any bytes are parsed as a GGUF file's header and
//! as a whole file. It must return for any input, and whatever is accepted
//! must keep the reader's promise: no tensor range ends past the file's end.
//!
//! A fuzz target calls it as is:
//!
//! ```ignore
//! #[path = "../../gguf/tests/harness/gguf_file.rs"]
//! mod harness;
//! libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::gguf_file(data));
//! ```
//!
//! Every sum is redone in u128, so an offset plus a size that wraps u64 shows
//! as past the end however the reader does its arithmetic.

#![allow(dead_code)]

use gguf::{Error, Gguf, Header, TensorInfo, byte_size};

pub fn gguf_file(data: &[u8]) {
    let len = data.len() as u64;
    let Ok(header) = Header::parse(data) else {
        assert!(Gguf::parse(data).is_err(), "a whole file without a header");
        return;
    };
    let align = header.alignment();
    assert!(align.is_power_of_two() && align >= 8, "alignment {align}");
    assert!(header.table_end() <= len, "the table ends past the bytes");
    assert!(header.data_offset() >= header.table_end());
    assert_eq!(header.data_offset() % align, 0);
    assert!(header.data_offset() - header.table_end() < align);
    assert!(header.tensors().len() as u64 <= gguf::MAX_TENSORS);
    // A name that parsed is unique: a duplicate is refused, never kept.
    let mut names = std::collections::HashSet::new();
    for info in header.tensors() {
        assert!(
            names.insert(info.name),
            "tensor {} appears twice",
            info.name
        );
    }

    let mut inside = true;
    for info in header.tensors() {
        // The size is the type's rule for the shape, never a number the file gave.
        assert_eq!(byte_size(info.ty, info.dims()), Ok(info.size));
        assert_eq!(info.offset % align, 0, "{}", info.name);
        let end = wide_end(&header, info);
        // Every file length that matters to this tensor: none, the header's,
        // the real one, and one byte either side of where the tensor ends.
        let lens = [
            0,
            header.table_end(),
            header.data_offset(),
            len,
            len.saturating_add(1),
            narrow(end.saturating_sub(1)),
            narrow(end),
            narrow(end + 1),
            u64::MAX,
        ];
        for file_len in lens {
            range_holds(&header, info, file_len);
        }
        inside &= end <= u128::from(len);
    }

    // `models pin` reads only the header's bytes: the table they hold is the same.
    let cut = &data[..header.table_end() as usize];
    let prefix = Header::parse(cut).expect("the header's own bytes parse");
    assert_eq!(prefix.tensors(), header.tensors());
    assert_eq!(prefix.data_offset(), header.data_offset());
    assert_eq!(prefix.check_extents(len).is_ok(), inside);

    match Gguf::parse(data) {
        Ok(file) => {
            assert!(inside, "a file with a tensor past its end was accepted");
            for info in header.tensors() {
                let tensor = file.tensor(info.name).expect("a checked tensor has data");
                let start = tensor.data.as_ptr() as usize - data.as_ptr() as usize;
                assert_eq!(start as u128, wide_start(&header, info));
                assert_eq!(tensor.data.len() as u64, info.size);
            }
        }
        Err(error) => {
            assert!(!inside, "every tensor inside, yet refused: {error:?}");
            assert_eq!(error, Error::TensorPastEnd);
        }
    }
}

/// `data_range` accepts exactly the tensors that end at or before `file_len`,
/// and gives the range the u128 sums give.
fn range_holds(header: &Header<'_>, info: &TensorInfo<'_>, file_len: u64) {
    let end = wide_end(header, info);
    match header.data_range(info, file_len) {
        Ok((start, stop)) => {
            assert!(
                end <= u128::from(file_len),
                "{} ends at {end}, past {file_len}",
                info.name
            );
            assert_eq!(u128::from(start), wide_start(header, info));
            assert_eq!(u128::from(stop), end);
        }
        Err(error) => {
            assert_eq!(error, Error::TensorPastEnd);
            assert!(
                end > u128::from(file_len),
                "{} ends at {end}, inside {file_len}",
                info.name
            );
        }
    }
}

fn wide_start(header: &Header<'_>, info: &TensorInfo<'_>) -> u128 {
    u128::from(header.data_offset()) + u128::from(info.offset)
}

fn wide_end(header: &Header<'_>, info: &TensorInfo<'_>) -> u128 {
    wide_start(header, info) + u128::from(info.size)
}

fn narrow(n: u128) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}
