//! Hostile mirror requests (P4 task 2): the kernel sends PKG_FETCH's request to
//! the `pkgrepo=` destination only if `kabi::pkgrepo::check` takes it. A mutated
//! or cut request is refused or is exactly one `pkg` could build: a GET of at
//! most 192 KiB of one file beneath `/metadata/` or `/targets/`.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kabi::pkgrepo::{MAX_RANGE, check, request, valid_path};

/// What a request `check` took must be.
fn only_a_range_of_one_file(bytes: &[u8]) {
    if check(bytes).is_err() {
        return;
    }
    let text = core::str::from_utf8(bytes).expect("taken, so text");
    let (path, rest) = text
        .strip_prefix("GET ")
        .and_then(|t| t.split_once(" HTTP/1.1\r\n"))
        .expect("taken, so a GET");
    assert!(valid_path(path), "{text:?}");
    let range = rest
        .split_once("Range: bytes=")
        .and_then(|(_, r)| r.split_once("\r\n"))
        .and_then(|(r, _)| r.split_once('-'))
        .expect("taken, so a range");
    let (first, last): (u64, u64) = (range.0.parse().unwrap(), range.1.parse().unwrap());
    assert!(first <= last && last - first < MAX_RANGE, "{text:?}");
    assert_eq!(request(path, first, last).unwrap().as_bytes(), bytes);
}

#[test]
fn mutated_requests_are_refused_or_exactly_a_range_of_one_file() {
    let sample = request("/targets/0a1b2c.notes-1.0.0.kpkg", 196_608, 393_215).unwrap();
    assert!(check(sample.as_bytes()).is_ok());
    hostile::fuzz(sample.as_bytes(), 0x7072, 5_000, only_a_range_of_one_file);
    hostile::truncations(sample.as_bytes(), |_, cut| only_a_range_of_one_file(cut));
    let escape = sample.replace("/targets/0a1b2c.notes-1.0.0.kpkg", "/targets/../../x");
    assert!(check(escape.as_bytes()).is_err());
}
