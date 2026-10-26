//! What an Allow on the menu grants (ADR 0001 D6): the digest the kernel holds
//! for a `ONCE` badge, `kai-once 2`, over the log entry id and the call exactly as
//! the menu showed it, in its canonical text ([`call_text`]). The terminal mints
//! with it, and whoever runs the call checks with it: the shell for its own
//! calls, the kernel for the calls it serves (it rebuilds the call from its own
//! arguments), so all of them use this one definition. SHA-256 is written out
//! here, small and portable, so the kernel needs no SIMD for it.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// One argument of a call, for its canonical text.
#[derive(Clone, Copy, Debug)]
pub enum CallArg<'a> {
    Text(&'a str),
    Int(i64),
}

/// A text argument as the call shows it: in double quotes, with `\` and `"`
/// escaped. `None` for one with a control character, which no call takes.
pub fn quote(text: &str) -> Option<String> {
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// The canonical text of a call: `process_stop("build-3")`,
/// `editor_open("notes/src/lib.rs", 40)`. `None` if an argument cannot be shown.
pub fn call_text(action: &str, args: &[CallArg]) -> Option<String> {
    let args: Option<Vec<String>> = args
        .iter()
        .map(|arg| match arg {
            CallArg::Text(text) => quote(text),
            CallArg::Int(n) => Some(format!("{n}")),
        })
        .collect();
    Some(format!("{action}({})", args?.join(", ")))
}

/// The `ONCE` digest (`kai-once 2`) of log entry `id`'s `call`, in canonical text.
pub fn digest_call(id: u64, call: &str) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(b"kai-once 2\n");
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    let mut rest = id;
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    sha.update(&digits[at..]);
    sha.update(b"\n");
    sha.update(call.as_bytes());
    sha.finish()
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 (FIPS 180-4), fed in pieces.
pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    bytes: u64,
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            filled: 0,
            bytes: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.bytes = self.bytes.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                let block = self.block;
                self.compress(&block);
                self.filled = 0;
            }
        }
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.bytes.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 32];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *chunk = word.to_be_bytes();
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*chunk);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> alloc::string::String {
        bytes.iter().map(|b| alloc::format!("{b:02x}")).collect()
    }

    fn sha(data: &[u8]) -> alloc::string::String {
        let mut sha = Sha256::new();
        sha.update(data);
        hex(&sha.finish())
    }

    #[test]
    fn sha256_matches_the_nist_vectors() {
        assert_eq!(
            sha(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let million = alloc::vec![b'a'; 1_000_000];
        assert_eq!(
            sha(&million),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn pieces_hash_as_the_whole() {
        let mut sha = Sha256::new();
        for piece in [
            &b"ab"[..],
            b"",
            b"cdbcdecdefdefgefghfghighijhijkijkljklmklmn",
            b"lmnomnopnopq",
        ] {
            sha.update(piece);
        }
        assert_eq!(
            hex(&sha.finish()),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn the_digest_names_the_entry_and_the_call_exactly() {
        let call = "process_stop(\"build-3\")";
        let d = digest_call(7, call);
        assert_eq!(d, digest_call(7, call));
        assert_ne!(d, digest_call(8, call));
        assert_ne!(d, digest_call(7, "process_stop(\"build-4\")"));
        assert_ne!(digest_call(1, "2x"), digest_call(12, "x"));
        // Pinned, so a kernel and a shell built apart agree.
        assert_eq!(hex(&d), sha(b"kai-once 2\n7\nprocess_stop(\"build-3\")"));
        assert_eq!(hex(&d), PINNED);
    }

    /// `kai-once 2` of entry 7's `process_stop("build-3")`.
    const PINNED: &str = "1ff1087a2d7dc10a7dfcbaf7257c1afb4b1b82d8f4dbe5e575e5f693a2bfc0ce";

    #[test]
    fn a_call_has_one_canonical_text() {
        let text = |action, args: &[CallArg]| call_text(action, args).unwrap();
        assert_eq!(
            text("process_stop", &[CallArg::Text("build-3")]),
            "process_stop(\"build-3\")"
        );
        assert_eq!(
            text(
                "editor_open",
                &[CallArg::Text("notes/src/lib.rs"), CallArg::Int(40)]
            ),
            "editor_open(\"notes/src/lib.rs\", 40)"
        );
        assert_eq!(text("cpus", &[]), "cpus()");
        assert_eq!(quote("a\"b\\c").unwrap(), "\"a\\\"b\\\\c\"");
        assert_eq!(call_text("x", &[CallArg::Text("a\nb")]), None);
    }
}
