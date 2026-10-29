//! The mirror's one request (ADR 0011 §8; P4 task 2): what PKG_FETCH sends to the
//! fixed `pkgrepo=` destination. It is a GET of one range of one file in the
//! repository's layout, `/metadata/<file>` or `/targets/<file>`. `pkg` builds it
//! with [`request`]; the kernel takes nothing else ([`check`]), so ring 3 never
//! chooses a destination, a method or a header.

use alloc::format;
use alloc::string::String;

/// The longest path a request names.
pub const MAX_PATH: usize = 160;
/// The most bytes one range asks for: a 192 KiB body keeps the whole response
/// under the kernel's 256 KiB limit (`llmproto::http::MAX_RESPONSE`).
pub const MAX_RANGE: u64 = 192 * 1024;
/// The folders a path may lie in.
const FOLDERS: [&str; 2] = ["/metadata/", "/targets/"];

/// Whether `path` names a file of the repository: one of [`FOLDERS`], then one
/// name of `[A-Za-z0-9._-]`, not `.` or `..`, at most [`MAX_PATH`] bytes in all.
pub fn valid_path(path: &str) -> bool {
    let Some(name) = FOLDERS.iter().find_map(|f| path.strip_prefix(f)) else {
        return false;
    };
    path.len() <= MAX_PATH
        && !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The GET of bytes `first..=last` of `path`.
pub fn request(path: &str, first: u64, last: u64) -> Result<String, &'static str> {
    if !valid_path(path) {
        return Err("not a path of the repository");
    }
    if last < first || last - first >= MAX_RANGE {
        return Err("a range is 1 byte to 192 KiB");
    }
    Ok(format!(
        "GET {path} HTTP/1.1\r\nHost: pkgrepo\r\nUser-Agent: kernel-ai\r\n\
         Range: bytes={first}-{last}\r\nConnection: close\r\n\r\n"
    ))
}

/// Whether `bytes` are exactly a request [`request`] builds: its path and range
/// are read back, and the request built from them must be the same bytes.
pub fn check(bytes: &[u8]) -> Result<(), &'static str> {
    let text = core::str::from_utf8(bytes).map_err(|_| "a request is not text")?;
    let path = text
        .strip_prefix("GET ")
        .and_then(|rest| rest.split_once(" HTTP/1.1\r\n"))
        .map(|(path, _)| path)
        .ok_or("not a GET")?;
    let range = text
        .split_once("\r\nRange: bytes=")
        .and_then(|(_, rest)| rest.split_once("\r\n"))
        .and_then(|(range, _)| range.split_once('-'))
        .ok_or("no range")?;
    let number = |digits: &str| -> Result<u64, &'static str> {
        let plain = !digits.is_empty()
            && digits.len() <= 20
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'));
        plain
            .then(|| digits.parse().ok())
            .flatten()
            .ok_or("a range is two plain numbers")
    };
    let built = request(path, number(range.0)?, number(range.1)?)?;
    match built.as_bytes() == bytes {
        true => Ok(()),
        false => Err("not the request pkg builds"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_reads_back_and_nothing_else_passes() {
        let good = request("/targets/0a1b.notes-1.0.0.kpkg", 0, MAX_RANGE - 1).unwrap();
        assert_eq!(check(good.as_bytes()), Ok(()));
        assert!(check(request("/metadata/1.root.json", 5, 5).unwrap().as_bytes()).is_ok());
        for (path, first, last) in [
            ("/metadata/../kernel", 0, 1),
            ("/metadata/", 0, 1),
            ("/metadata/a/b", 0, 1),
            ("/etc/passwd", 0, 1),
            ("metadata/x", 0, 1),
            ("/targets/x y", 0, 1),
            ("/targets/x", 1, 0),
            ("/targets/x", 0, MAX_RANGE),
        ] {
            assert!(request(path, first, last).is_err(), "{path} {first}-{last}");
        }
        let bend = |from: &str, to: &str| check(good.replacen(from, to, 1).as_bytes());
        for (from, to) in [
            ("GET", "POST"),
            ("Host: pkgrepo", "Host: example.com"),
            ("bytes=0-", "bytes=00-"),
            ("bytes=0-", "bytes=+0-"),
            (
                "Connection: close\r\n",
                "Connection: close\r\nX-Extra: 1\r\n",
            ),
            ("HTTP/1.1", "HTTP/1.0"),
        ] {
            assert!(bend(from, to).is_err(), "{from} -> {to}");
        }
        assert!(check(&[good.as_bytes(), b"body"].concat()).is_err());
        assert!(check(&good.as_bytes()[..good.len() - 2]).is_err());
    }
}
