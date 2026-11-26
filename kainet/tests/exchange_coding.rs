//! `Fetch::exchange`'s content codings (plan P6 F-4): gzip and brotli undone
//! in any pieces, broken streams refused, and a bomb stopped at the limit.

#[path = "exchange/kit.rs"]
mod kit;

use kainet::http::{Coding, Fetch, HttpError, Limits, Request, Verb};
use kit::*;

/// gzip, with a name, a comment, an extra field and a header CRC, and brotli,
/// are undone in any pieces; what reaches the sink is the page.
#[test]
fn gzip_and_brotli_bodies_are_decoded_in_any_pieces() {
    let at = url("https://a.test/page");
    let text = page(MIB + 5);
    let mut fancy = gzip_with_fields(&text);
    let plain = gzip(&text);
    let br = brotli(&text);
    for (coding, coded, name) in [
        (Coding::Gzip, plain.clone(), "gzip"),
        (Coding::Gzip, plain, "x-gzip"),
        (Coding::Gzip, std::mem::take(&mut fancy), "GZIP"),
        (Coding::Brotli, br, "br"),
    ] {
        assert!(coded.len() < text.len() / 10, "{name} codes the page");
        let header = format!("Content-Encoding: {name}");
        for step in [1 << 20, 1000, 7] {
            for wire in [
                reply("200 OK", &[&header], &coded),
                chunked("200 OK", &[&header], &coded, 333),
            ] {
                let mut server = Server::owned(vec![vec![wire]]).stepped(step);
                let (answer, got) = run(&mut server, ask(Verb::Get, &at), 2 * MIB as u64);
                let answer = answer.unwrap_or_else(|e| panic!("{name} step {step}: {e:?}"));
                assert_eq!(answer.coding, coding);
                assert_eq!(answer.received, text.len() as u64);
                assert_eq!(answer.wire, coded.len() as u64);
                assert!(got == text, "{name} step {step}");
            }
        }
    }
    // Asked for and identity, or no coding: passed as it is.
    let mut server = Server::new(&[&[
        b"HTTP/1.1 200 OK\r\nContent-Encoding: identity\r\nContent-Length: 2\r\n\r\nhi",
    ]]);
    assert_eq!(run(&mut server, ask(Verb::Get, &at), 10).1, b"hi");
    // An empty coded body is empty, as browsers take it.
    let mut server = Server::new(&[&[
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n",
    ]]);
    let (answer, got) = run(&mut server, ask(Verb::Get, &at), 10);
    assert_eq!((answer.unwrap().received, got.len()), (0, 0));
}

/// A gzip member with every optional header field and a header CRC.
fn gzip_with_fields(body: &[u8]) -> Vec<u8> {
    let plain = gzip(body);
    // FHCRC | FEXTRA | FNAME | FCOMMENT, the fixed bytes otherwise flate2's.
    let mut header = plain[..10].to_vec();
    header[3] = 0x02 | 0x04 | 0x08 | 0x10;
    header.extend_from_slice(&5u16.to_le_bytes());
    header.extend_from_slice(b"KA\x01\x00z");
    header.extend_from_slice(b"page.html\0");
    header.extend_from_slice(b"a comment\0");
    let crc = crc32(&header) as u16;
    header.extend_from_slice(&crc.to_le_bytes());
    header.extend_from_slice(&plain[10..]);
    header
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut gz = flate2::Crc::new();
    gz.update(bytes);
    gz.sum()
}

/// A small coded body that decodes without end stops at the limit on decoded
/// bytes, holding no more than its buffers; broken codings are refused.
#[test]
fn hostile_codings_are_refused_and_a_bomb_stops_at_the_limit() {
    let at = url("https://a.test/bomb");
    let zeros = vec![0u8; 16 * MIB];
    for (name, coded) in [("gzip", gzip(&zeros)), ("br", brotli(&zeros))] {
        assert!(
            coded.len() < 200 * 1024,
            "{name} is a bomb: {}",
            coded.len()
        );
        let header = format!("Content-Encoding: {name}");
        let mut server = Server::owned(vec![vec![reply("200 OK", &[&header], &coded)]]);
        let mut passed = 0u64;
        let fetch = Fetch {
            limits: Limits::new(MIB as u64, u64::MAX),
            allow: &anyone,
            clock: &zero,
        };
        let got = fetch.exchange(&mut server, ask(Verb::Get, &at), &mut |bytes| {
            passed += bytes.len() as u64;
            Ok(())
        });
        assert_eq!(got, Err(HttpError::TooLarge), "{name}");
        assert!(passed <= MIB as u64, "{name}: {passed}");
    }
    let text = page(100_000);
    let good = gzip(&text);
    let mut bad_crc = good.clone();
    let n = bad_crc.len();
    bad_crc[n - 8] ^= 1;
    let mut bad_size = good.clone();
    bad_size[n - 1] ^= 1;
    let mut bad_deflate = good.clone();
    bad_deflate[12] ^= 0xFF;
    let mut reserved = good.clone();
    reserved[3] = 0x20;
    let mut not_deflate = good.clone();
    not_deflate[2] = 7;
    let mut bad_header_crc = gzip_with_fields(&text);
    let crc_at = 10 + 2 + 5 + 10 + 10;
    bad_header_crc[crc_at] ^= 1;
    let mut trailing = good.clone();
    trailing.extend_from_slice(&good);
    let mut endless_name = good[..10].to_vec();
    endless_name[3] = 0x08;
    endless_name.extend(std::iter::repeat_n(b'n', 5000));
    let br = brotli(&text);
    let mut br_trailing = br.clone();
    br_trailing.extend_from_slice(b"more");
    for (name, coding, body) in [
        ("a bad CRC", "gzip", bad_crc),
        ("a bad size", "gzip", bad_size),
        ("broken deflate", "gzip", bad_deflate),
        ("a reserved flag", "gzip", reserved),
        ("a method not deflate", "gzip", not_deflate),
        ("a bad header CRC", "gzip", bad_header_crc),
        ("a second member", "gzip", trailing),
        ("a name without end", "gzip", endless_name),
        ("cut short", "gzip", good[..good.len() - 3].to_vec()),
        ("not gzip", "gzip", text[..100].to_vec()),
        ("not brotli", "br", text[..100].to_vec()),
        ("brotli cut short", "br", br[..br.len() / 2].to_vec()),
        ("bytes after brotli", "br", br_trailing),
    ] {
        let header = format!("Content-Encoding: {coding}");
        let mut server = Server::owned(vec![vec![reply("200 OK", &[&header], &body)]]);
        let got = run(&mut server, ask(Verb::Get, &at), 4 * MIB as u64).0;
        assert!(matches!(got, Err(HttpError::Body(_))), "{name}: {got:?}");
    }
    // Codings it did not ask for, more than one, or ones it does not know.
    for (decode, header) in [
        (false, "Content-Encoding: gzip"),
        (false, "Content-Encoding: br"),
        (true, "Content-Encoding: deflate"),
        (true, "Content-Encoding: compress"),
        (true, "Content-Encoding: zstd"),
        (true, "Content-Encoding: gzip, br"),
        (true, "Content-Encoding: gzip\r\nContent-Encoding: gzip"),
    ] {
        let mut server = Server::owned(vec![vec![reply("200 OK", &[header], &good)]]);
        let request = Request {
            decode,
            ..ask(Verb::Get, &at)
        };
        let got = run(&mut server, request, MIB as u64).0;
        assert!(matches!(got, Err(HttpError::Head(_))), "{header}: {got:?}");
    }
}
