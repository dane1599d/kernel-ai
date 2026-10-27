//! The test-signed mirror (P4 task 2's host half; ADR 0011 §8): a repository
//! served from memory with the layout `kernel-ai-updates` will have,
//! `metadata/` and `targets/`. It answers GET and HEAD, and a single
//! `Range: bytes=<first>-[<last>]` with 206 Partial Content, which is how
//! `pkg` downloads: pieces of at most [`kaipkg::store::PIECE`], so each
//! response stays under llm's 256 KiB limit. QEMU's user networking maps the
//! guest's 10.0.2.2 to the host's 127.0.0.1, so a guest reaches it there.

use super::Repository;
use crate::httpd::{self, HttpError};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

/// Called for each range served, with the file's path and the `[start, end)`
/// bytes sent.
pub type OnRange = Arc<dyn Fn(&str, usize, usize) + Send + Sync>;

/// Serves `repository` on 127.0.0.1:`port` (0 for a free one) until xtask
/// exits, calling `on_range` for each range. Ok is the port.
pub fn start(repository: Repository, port: u16, on_range: OnRange) -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("cannot start the mirror on port {port}: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("the mirror has no address: {e}"))?
        .port();
    let files: Arc<BTreeMap<String, Vec<u8>>> = Arc::new(repository.files.into_iter().collect());
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let (files, on_range) = (Arc::clone(&files), Arc::clone(&on_range));
            thread::spawn(move || {
                if let Err(error) = serve(&stream, &files, on_range.as_ref()) {
                    eprintln!("xtask: mirror: {error}");
                }
            });
        }
    });
    Ok(port)
}

fn serve(
    stream: &TcpStream,
    files: &BTreeMap<String, Vec<u8>>,
    on_range: &(dyn Fn(&str, usize, usize) + Send + Sync),
) -> io::Result<()> {
    let request = match httpd::read_request(stream, 0, |r| {
        matches!(r.method.as_str(), "GET" | "HEAD")
    }) {
        Ok(request) => request,
        Err(HttpError::Io(error)) => return Err(error),
        Err(HttpError::Forbidden) => {
            return reply(stream, "405 Method Not Allowed", &[], b"", false);
        }
        Err(HttpError::TooLarge) => return reply(stream, "413 Payload Too Large", &[], b"", false),
        Err(HttpError::Malformed(_)) => return reply(stream, "400 Bad Request", &[], b"", false),
    };
    let head = request.method == "HEAD";
    let Some((path, bytes)) = request
        .path
        .strip_prefix('/')
        .and_then(|path| files.get_key_value(path))
    else {
        return reply(stream, "404 Not Found", &[], b"", head);
    };
    let Some(spec) = request.header("range") else {
        return reply(
            stream,
            "200 OK",
            &[("Accept-Ranges", "bytes".into())],
            bytes,
            head,
        );
    };
    match range(spec, bytes.len()) {
        Some((start, end)) => {
            on_range(path, start, end);
            let content = format!("bytes {start}-{}/{}", end - 1, bytes.len());
            reply(
                stream,
                "206 Partial Content",
                &[("Content-Range", content)],
                &bytes[start..end],
                head,
            )
        }
        None => {
            let content = format!("bytes */{}", bytes.len());
            reply(
                stream,
                "416 Range Not Satisfiable",
                &[("Content-Range", content)],
                b"",
                head,
            )
        }
    }
}

/// A single `bytes=<first>-[<last>]` range of a file of `len` bytes, as
/// `[start, end)`, the end clipped to the file. A suffix range, several
/// ranges, or one that starts past the end is None.
pub fn range(spec: &str, len: usize) -> Option<(usize, usize)> {
    let number = |text: &str| -> Option<usize> {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    let (first, last) = spec.strip_prefix("bytes=")?.split_once('-')?;
    let start = number(first)?;
    let end = match last {
        "" => len,
        last => number(last)?.checked_add(1)?.min(len),
    };
    (start < end).then_some((start, end))
}

fn reply(
    mut stream: &TcpStream,
    status: &str,
    headers: &[(&str, String)],
    body: &[u8],
    head: bool,
) -> io::Result<()> {
    let mut text = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("Connection: close\r\n\r\n");
    stream.write_all(text.as_bytes())?;
    if !head {
        stream.write_all(body)?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests;
