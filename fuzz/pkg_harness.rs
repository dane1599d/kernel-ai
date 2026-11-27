//! The fuzz targets' bodies for what `pkg` and `updated` read from a
//! repository (ADR 0011 §1 and §3, ADR 0008 §7; ADR 0000 P8): `kaipkg`'s
//! package reader, a `.kpkg`'s `kai-pkg 1` files, its `kai-package 1` header
//! and an app's artifact, and `kaituf`'s metadata, root, timestamp, snapshot
//! and targets as signed JSON, read on their own and through the image check
//! over a repository with one file replaced. Each `fuzz_targets/*.rs` calls one
//! with libFuzzer's input, and `kaipkg/tests/fuzz_harness.rs` includes this file
//! and runs each over hostile mutations of the samples here. Each must return
//! for any input; only a bug in the code under it may panic.

#![allow(dead_code)]

use kaipkg::bundle::{self, HEADER};
use kaipkg::generation::Time;
use kaipkg::header::{Header, Kind};
use kaipkg::image::{self, Files};
use kaipkg::sign::{self, Key, Keys, Periods, Target};
use kaipkg::version::{Range, Version};
use kaipkg::{app, effects};
use kaituf::meta::{Meta, Root, Snapshot, Targets, Timestamp};
use kaituf::trusted::TrustedSet;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// A data package's header.
pub const DATA: &str = "kai-package 1\nname hello\nversion 1.0.0\nkind data\n\
    summary A greeting\nlicence MIT\n";
/// An app package's header.
pub const APP: &str = "kai-package 1\nname notes\nversion 2.1.0\nkind app\n\
    summary Notes\nlicence MIT OR Apache-2.0\nworld kai:app@0.1\ndepends kai:app ^0.1\n";
/// The app's artifact's manifest (`livecore::manifest`).
const MANIFEST: &str = "kai-manifest 2\nlifecycle 1\ncomponent notes\nversion 2-1-0\n\
    world app:notes/notes@2.1.0\nengine wasmi\nfeatures -\nlicence MIT OR Apache-2.0\n\
    build ab12\nsource cd34\n";

/// `section` as a wasm custom section named `name`, after `module`.
fn custom(module: &mut Vec<u8>, name: &str, payload: &[u8]) {
    fn leb(out: &mut Vec<u8>, mut n: usize) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            match n {
                0 => return out.push(byte),
                _ => out.push(byte | 0x80),
            }
        }
    }
    let mut body = Vec::new();
    leb(&mut body, name.len());
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(payload);
    module.push(0);
    leb(module, body.len());
    module.extend_from_slice(&body);
}

/// An app's artifact: a module with its manifest and source sections.
pub fn artifact() -> Vec<u8> {
    let mut module = b"\0asm\x01\0\0\0".to_vec();
    module.extend_from_slice(&[1, 4, 1, 0x60, 0, 0]);
    custom(&mut module, "kai.manifest", MANIFEST.as_bytes());
    custom(
        &mut module,
        "kai.source",
        b"kai-source 1\nfile src/main.c 13\nint main(){}\n",
    );
    module
}

/// The two sample packages: a data package with a payload, and an app with
/// its artifact.
pub fn packages() -> Vec<Vec<u8>> {
    vec![
        bundle::build(&[(HEADER, DATA.as_bytes()), ("payload", b"hello there")]),
        bundle::build(&[(HEADER, APP.as_bytes()), (bundle::ARTIFACT, &artifact())]),
    ]
}

/// A `.kpkg` from a repository, as `pkg` reads it: its files, then its header
/// and, for an app, its artifact's component and effects. What `files` takes
/// is canonical: building its files again gives the same bytes. The input is
/// also read as a header on its own.
pub fn kpkg(bytes: &[u8]) {
    if let Ok(text) = core::str::from_utf8(bytes) {
        header(text);
    }
    let Ok(files) = bundle::files(bytes) else {
        return;
    };
    assert!(files.len() <= bundle::MAX_FILES, "{} files", files.len());
    assert_eq!(files[0].0, HEADER);
    assert_eq!(
        bundle::build(&files),
        bytes,
        "a package read is not as built"
    );
    let Ok(text) = core::str::from_utf8(files[0].1) else {
        return;
    };
    let Some(header) = header(text) else {
        return;
    };
    let service = app::service(&header, bytes);
    let declared = effects::declared(&header, bytes);
    if header.kind != Kind::App {
        assert_eq!(service, Ok(None));
        assert_eq!(declared, Ok(Vec::new()));
    }
}

/// `text` as a header; one it takes keeps its version and ranges as written.
fn header(text: &str) -> Option<Header> {
    let header = Header::parse(text).ok()?;
    let version = header.version.to_string();
    assert_eq!(Version::parse(&version), Ok(header.version), "{version}");
    for depends in &header.depends {
        let range = depends.range;
        // The same range, written as its comparisons.
        let text = match range.high {
            Some(high) => format!(">={} <{high}", range.low),
            None => format!(">={}", range.low),
        };
        assert_eq!(Range::parse(&text), Ok(range), "{depends:?}");
    }
    Some(header)
}

/// The sample repository, as the image carries it: metadata and both sample
/// packages at `targets/<name>-<version>.kpkg`, signed by one key.
pub fn repository() -> &'static BTreeMap<String, Vec<u8>> {
    static REPO: OnceLock<BTreeMap<String, Vec<u8>>> = OnceLock::new();
    REPO.get_or_init(|| {
        let key = Key::from_seed([0x4b; 32]);
        let targets: Vec<Target> = packages()
            .into_iter()
            .map(|bytes| {
                let files = bundle::files(&bytes).expect("a sample package");
                let text = core::str::from_utf8(files[0].1).expect("a header");
                let header = Header::parse(text).expect("a sample header");
                Target {
                    path: format!("{}-{}.kpkg", header.name, header.version),
                    kai: index(&header, text),
                    bytes,
                }
            })
            .collect();
        let periods = Periods {
            root: 3650,
            targets: 3650,
            snapshot: 3650,
            timestamp: 3650,
        };
        let built = sign::build(&targets, &Keys::one(&key), Time(1_800_000_000), periods)
            .expect("a sample repository");
        built
            .into_iter()
            .map(|(path, bytes)| {
                // The image's layout: a target without its SHA-256 before it.
                let path = match path.strip_prefix("targets/") {
                    Some(rest) => format!("targets/{}", rest.split_once('.').map_or(rest, |r| r.1)),
                    None => path,
                };
                (path, bytes)
            })
            .collect()
    })
}

/// `header`'s `custom.kai` index fields, as `local` writes them.
fn index(header: &Header, text: &str) -> Value {
    let depends: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("depends "))
        .collect();
    let mut kai = json!({
        "name": header.name,
        "version": header.version.to_string(),
        "kind": header.kind.name(),
        "summary": header.summary,
        "licence": header.licence,
        "depends": depends,
        "grants": [],
        "actions": [],
        "yanked": false,
    });
    if let Some(world) = &header.world {
        kai["world"] = json!(world);
    }
    kai
}

/// What the OS provides the sample app.
pub const PROVIDED: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];

/// The sample repository with `replaced` in place of its file `path`.
struct Replaced<'a> {
    path: &'a str,
    replaced: &'a [u8],
}

impl Files for Replaced<'_> {
    fn file(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let bytes = match path == self.path {
            true => Some(self.replaced),
            false => repository().get(path).map(Vec::as_slice),
        };
        match bytes {
            Some(bytes) if bytes.len() as u64 > max => Err(format!("{path} is too long")),
            bytes => Ok(bytes.map(<[u8]>::to_vec)),
        }
    }
}

/// The image check over the sample repository, every package kept.
pub fn check_sample() -> image::Checked {
    let mut files = Replaced {
        path: "",
        replaced: &[],
    };
    image::check(&mut files, PROVIDED).expect("the sample repository verifies")
}

/// TUF metadata from a repository: `bytes` as each role, and, with its first
/// byte naming one of the sample repository's files, the rest in that file's
/// place under the image check. Each role it takes has canonical bytes that
/// parse as JSON and come back the same.
pub fn tuf_metadata(bytes: &[u8]) {
    canonical(Root::parse(bytes));
    canonical(Timestamp::parse(bytes));
    canonical(Snapshot::parse(bytes));
    canonical(Targets::parse(bytes));
    let _ = TrustedSet::new(bytes, 1_800_000_000);
    let Some((&which, rest)) = bytes.split_first() else {
        return;
    };
    let paths: Vec<&String> = repository().keys().collect();
    let path = paths[usize::from(which) % paths.len()];
    let mut files = Replaced {
        path,
        replaced: rest,
    };
    if let Ok(checked) = image::check(&mut files, PROVIDED) {
        // Only what the signed metadata names is kept, each package whole.
        for (entry, bytes) in &checked.packages {
            assert_eq!(kaipkg::sha256(bytes), entry.sha, "{}", entry.name);
            assert!(bundle::files(bytes).is_ok(), "{}", entry.name);
        }
    }
}

fn canonical<T>(meta: Result<Meta<T>, kaituf::Error>) {
    let Ok(meta) = meta else {
        return;
    };
    let value: Value = serde_json::from_slice(&meta.canonical).expect("canonical JSON parses");
    let again = kaituf::canonical::canonical(&value).expect("canonical JSON is canonical");
    assert_eq!(again, meta.canonical, "canonical bytes are not stable");
}

/// Each target's starting corpus: the sample packages and headers, and each
/// metadata file of the sample repository behind the byte that names it.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let mut seeds: Vec<(&'static str, Vec<u8>)> =
        packages().into_iter().map(|p| ("kpkg", p)).collect();
    seeds.push(("kpkg", DATA.as_bytes().to_vec()));
    seeds.push(("kpkg", APP.as_bytes().to_vec()));
    for (i, (path, bytes)) in repository().iter().enumerate() {
        if path.starts_with("metadata/") {
            let mut input = vec![i as u8];
            input.extend_from_slice(bytes);
            seeds.push(("tuf_metadata", input));
        }
    }
    seeds
}
