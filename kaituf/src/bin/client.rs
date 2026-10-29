//! tuf-conformance's client-under-test CLI (its CLIENT-CLI.md, v2.5.0) over
//! `kaituf`, for CI only:
//!
//! ```text
//! kaituf-client --metadata-dir DIR init TRUSTED_ROOT
//! kaituf-client --metadata-dir DIR --metadata-url URL refresh
//! kaituf-client --metadata-dir DIR --metadata-url URL --target-name PATH...
//!     --target-base-url URL --target-dir DIR download
//! kaituf-client canonical FILE | serde FILE | parse FILE
//! ```
//!
//! Metadata is kept in DIR as `<role>.json`, the role name quoted, with the bytes
//! the repository served. An artifact is kept in the target dir under its quoted
//! path, and not fetched again while it still matches. Plain HTTP only: the suite
//! serves 127.0.0.1. The time is the system's, so the suite's faketime moves it.
//!
//! For the spikes: `canonical` and `serde` print FILE's `signed` as kaituf's
//! canonical form and as serde_json's compact one (S08-3's comparison), and
//! `parse` reads FILE as the role its `_type` names.

use kaituf::updater::{Cache, Remote, Updater, check_target};
use kaituf::{Error, quote};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs};

/// The most a response's headers may take.
const MAX_HEAD: usize = 64 << 10;
/// The most an artifact may be, beyond what its metadata says.
const MAX_TARGET: u64 = 1 << 30;

#[derive(Default)]
struct Args {
    metadata_dir: Option<PathBuf>,
    metadata_url: Option<String>,
    target_names: Vec<String>,
    target_base_url: Option<String>,
    target_dir: Option<PathBuf>,
    words: Vec<String>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--metadata-dir" => out.metadata_dir = Some(value()?.into()),
            "--metadata-url" => out.metadata_url = Some(value()?),
            "--target-name" => out.target_names.push(value()?),
            "--target-base-url" => out.target_base_url = Some(value()?),
            "--target-dir" => out.target_dir = Some(value()?.into()),
            _ => out.words.push(arg),
        }
    }
    Ok(out)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("kaituf-client: {why}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse(env::args().skip(1))?;
    let words: Vec<&str> = args.words.iter().map(String::as_str).collect();
    let dir = || args.metadata_dir.clone().ok_or("no --metadata-dir");
    match words.as_slice() {
        ["init", root] => {
            let bytes = fs::read(root).map_err(|e| format!("{root}: {e}"))?;
            Files(dir()?)
                .save("root", &bytes)
                .map_err(|e| e.to_string())
        }
        ["refresh"] => {
            let (mut remote, mut cache) = (http(&args.metadata_url)?, Files(dir()?));
            let mut updater =
                Updater::new(&mut remote, &mut cache, now()).map_err(|e| e.to_string())?;
            updater.refresh().map_err(|e| e.to_string())
        }
        ["download"] => download(&args, dir()?),
        [form @ ("canonical" | "serde"), file] => {
            let bytes = fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let out = match *form {
                "canonical" => {
                    kaituf::canonical::canonical(&value["signed"]).map_err(|e| e.to_string())?
                }
                _ => serde_json::to_vec(&value["signed"]).map_err(|e| e.to_string())?,
            };
            std::io::stdout().write_all(&out).map_err(|e| e.to_string())
        }
        ["parse", file] => parse_any(file),
        _ => Err(
            "usage: init ROOT | refresh | download | canonical FILE | serde FILE | parse FILE"
                .into(),
        ),
    }
}

/// Whether `file` parses as the role its `_type` says, printing that and its version.
fn parse_any(file: &str) -> Result<(), String> {
    use kaituf::meta::{Root, Snapshot, Targets, Timestamp};
    let bytes = fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let kind = value["signed"]["_type"].as_str().unwrap_or("");
    let version = match kind {
        "root" => Root::parse(&bytes).map(|m| m.version),
        "timestamp" => Timestamp::parse(&bytes).map(|m| m.version),
        "snapshot" => Snapshot::parse(&bytes).map(|m| m.version),
        "targets" => Targets::parse(&bytes).map(|m| m.version),
        other => return Err(format!("{file}: unknown _type {other:?}")),
    };
    let version = version.map_err(|e| format!("{file}: {e}"))?;
    println!("{kind} {version}");
    Ok(())
}

fn download(args: &Args, metadata: PathBuf) -> Result<(), String> {
    let base = args.target_base_url.clone().ok_or("no --target-base-url")?;
    let into = args.target_dir.clone().ok_or("no --target-dir")?;
    let (mut remote, mut cache) = (http(&args.metadata_url)?, Files(metadata));
    let mut updater = Updater::new(&mut remote, &mut cache, now()).map_err(|e| e.to_string())?;
    updater.refresh().map_err(|e| e.to_string())?;
    for name in &args.target_names {
        let target = updater.target(name).map_err(|e| e.to_string())?;
        let target = target.ok_or(format!("no trusted role names {name}"))?;
        let local = into.join(quote(name));
        if fs::read(&local).is_ok_and(|bytes| check_target(&target, &bytes).is_ok()) {
            continue;
        }
        let url = join(&base, &updater.target_url(&target));
        let bytes = get(&url, target.length.min(MAX_TARGET))
            .map_err(|e| e.to_string())?
            .ok_or(format!("{url}: not found"))?;
        check_target(&target, &bytes).map_err(|e| e.to_string())?;
        write_atomic(&local, &bytes)?;
    }
    Ok(())
}

/// The system's time, or `KAITUF_NOW` (seconds since 1970) where faketime is
/// not at hand.
fn now() -> u64 {
    if let Some(secs) = env::var("KAITUF_NOW").ok().and_then(|s| s.parse().ok()) {
        return secs;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `base` and `name` with one `/` between them.
fn join(base: &str, name: &str) -> String {
    format!("{}/{name}", base.trim_end_matches('/'))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let partial = path.with_extension("partial");
    fs::write(&partial, bytes)
        .and_then(|()| fs::rename(&partial, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The metadata directory, one `<quoted role>.json` per role.
struct Files(PathBuf);

impl Cache for Files {
    fn load(&mut self, role: &str) -> Result<Option<Vec<u8>>, Error> {
        let path = self.0.join(format!("{}.json", quote(role)));
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::Io(format!("{}: {e}", path.display()))),
        }
    }

    fn save(&mut self, role: &str, bytes: &[u8]) -> Result<(), Error> {
        write_atomic(&self.0.join(format!("{}.json", quote(role))), bytes).map_err(Error::Io)
    }
}

struct Http(String);

fn http(url: &Option<String>) -> Result<Http, String> {
    Ok(Http(url.clone().ok_or("no --metadata-url")?))
}

impl Remote for Http {
    fn fetch(&mut self, name: &str, max: u64) -> Result<Option<Vec<u8>>, Error> {
        get(&join(&self.0, name), max)
    }
}

/// `url`'s body, at most `max` bytes; None for 404 and 403.
fn get(url: &str, max: u64) -> Result<Option<Vec<u8>>, Error> {
    let io = |e: std::io::Error| Error::Io(format!("{url}: {e}"));
    let rest = url
        .strip_prefix("http://")
        .ok_or(Error::Io(format!("{url}: not http://")))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut stream = TcpStream::connect(host).map_err(io)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(io)?;
    let request = format!(
        "GET /{} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept-Encoding: identity\r\n\r\n",
        escape(path)
    );
    stream.write_all(request.as_bytes()).map_err(io)?;
    let mut raw = Vec::new();
    let limit = max.saturating_add(MAX_HEAD as u64 + 1);
    stream.take(limit).read_to_end(&mut raw).map_err(io)?;
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n");
    let end = end.ok_or(Error::Io(format!("{url}: no end to the response's head")))?;
    let head = String::from_utf8_lossy(&raw[..end]).to_lowercase();
    let body = raw.split_off(end + 4);
    let status = head.split(' ').nth(1).unwrap_or("");
    match status {
        "200" => {}
        "403" | "404" => return Ok(None),
        other => return Err(Error::Io(format!("{url}: HTTP {other}"))),
    }
    if head.contains("transfer-encoding: chunked") {
        return Err(Error::Io(format!("{url}: chunked responses are not read")));
    }
    if body.len() as u64 > max {
        return Err(Error::TooLarge(url.into()));
    }
    Ok(Some(body))
}

/// `path` with every byte outside the characters a URL path may hold as they
/// are percent-encoded; ordinary names are unchanged.
fn escape(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/%!$&'()*+,;=:@".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
