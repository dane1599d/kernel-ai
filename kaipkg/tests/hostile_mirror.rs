//! A hostile mirror (P4 task 2): the transport is never trusted. Whatever bytes
//! come back for a range, mutated or cut, `kaipkg::mirror` refuses them or gives
//! at most the bytes it asked for, and never panics; a whole file never runs
//! past its cap.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kaipkg::mirror::{Exchange, Mirror};
use kaipkg::store::PIECE;

/// Answers every request with the same bytes.
struct Canned(Vec<u8>);

impl Exchange for Canned {
    fn exchange(&mut self, _: &[u8]) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }
}

/// Answers every range with exactly the bytes it asks for, as if the file never
/// ended.
struct Endless;

impl Exchange for Endless {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, String> {
        let text = core::str::from_utf8(request).unwrap();
        let (_, range) = text.split_once("bytes=").unwrap();
        let (first, rest) = range.split_once('-').unwrap();
        let last = rest.split_once('\r').unwrap().0;
        let len = last.parse::<usize>().unwrap() - first.parse::<usize>().unwrap() + 1;
        Ok(answer("206 Partial Content", &vec![3; len]))
    }
}

fn answer(status: &str, body: &[u8]) -> Vec<u8> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/9999\r\n\r\n",
        body.len(),
        body.len().saturating_sub(1)
    );
    [head.as_bytes(), body].concat()
}

/// A range of `want` bytes from these response bytes is refused or at most `want`.
fn bounded(response: &[u8]) {
    for want in [1, 16, 64] {
        let mut mirror = Mirror::new(Canned(response.to_vec()));
        if let Ok(Some(got)) = mirror.range("/targets/0a.x-1.0.0.kpkg", 3, want) {
            assert!(got.len() <= want, "{} bytes for {want}", got.len());
        }
        let mut mirror = Mirror::new(Canned(response.to_vec()));
        if let Ok(Some(got)) = mirror.whole("/metadata/timestamp.json", 40) {
            assert!(got.len() <= 41, "{} bytes past a cap of 40", got.len());
        }
    }
}

#[test]
fn mutated_answers_are_refused_or_bounded() {
    let partial = answer("206 Partial Content", b"0123456789abcdef");
    hostile::fuzz(&partial, 0x6d69, 4_000, bounded);
    hostile::truncations(&partial, |_, cut| bounded(cut));
    let whole = answer("200 OK", &[b'z'; 300]);
    hostile::fuzz(&whole, 0x6d6a, 2_000, bounded);
}

#[test]
fn a_mirror_that_sends_more_than_asked_or_never_ends_is_held_to_its_cap() {
    // 206 with more than the range: refused.
    let mut greedy = Mirror::new(Canned(answer("206 Partial Content", &[1; 64])));
    assert!(greedy.range("/targets/x", 0, 16).is_err());
    let mut greedy = Mirror::new(Canned(answer("206 Partial Content", &vec![2; PIECE])));
    assert!(greedy.whole("/metadata/1.root.json", 1000).is_err());
    // Every range it is asked for, without end: a whole file stops one byte past
    // its cap, in pieces.
    for cap in [1000, 3 * PIECE as u64 + 5] {
        let mut endless = Mirror::new(Endless);
        let got = endless
            .whole("/metadata/1.root.json", cap)
            .unwrap()
            .unwrap();
        assert_eq!(got.len() as u64, cap + 1);
        assert_eq!(endless.requests as u64, (cap + 1).div_ceil(PIECE as u64));
    }
    // Not found, and an answer that is not HTTP.
    let mut absent = Mirror::new(Canned(answer("404 Not Found", b"")));
    assert_eq!(absent.whole("/metadata/2.root.json", 1000), Ok(None));
    let mut junk = Mirror::new(Canned(b"SSH-2.0-OpenSSH\r\n".to_vec()));
    assert!(junk.range("/metadata/1.root.json", 0, 10).is_err());
    // A path or range outside the repository is never sent.
    let mut mirror = Mirror::new(Canned(answer("206 Partial Content", b"x")));
    assert!(mirror.range("/../kernel.lines", 0, 1).is_err());
    assert_eq!(mirror.requests, 0);
}
