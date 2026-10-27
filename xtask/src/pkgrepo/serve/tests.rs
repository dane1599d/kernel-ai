use super::*;
use crate::pkgrepo::tests::{NOW, keys, notes};
use crate::pkgrepo::{Package, tuf};
use kaipkg::bundle;
use kaipkg::header::{Depends, Header};
use kaipkg::resolve::{Entry, Want};
use kaipkg::store::{self, Ctx, Error, Ledger, PIECE, Refused, Source, Store, Volume};
use kaipkg::version::{Range, Version};
use kaipkg::{Sha, unhex};
use serde_json::Value;
use std::io::Read;
use std::sync::Mutex;

/// One range served: the file's path and the `[start, end)` bytes sent.
type Served = (String, usize, usize);

/// A mirror of `repo` and what it has served so far.
fn mirror(repo: Repository) -> (u16, Arc<Mutex<Vec<Served>>>) {
    let served = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&served);
    let on_range: OnRange = Arc::new(move |path: &str, start, end| {
        log.lock().unwrap().push((path.to_string(), start, end));
    });
    (start(repo, 0, on_range).unwrap(), served)
}

/// A `data` package of `len` bytes of payload, big enough to take pieces.
fn data(len: usize) -> Package {
    let header_text = "kai-package 1\nname big-data\nversion 0.1.0\nkind data\n\
        summary Test data in several pieces\nlicence MIT\n"
        .to_string();
    let bytes = bundle::build(&[
        (bundle::HEADER, header_text.as_bytes()),
        ("blob", &vec![7; len]),
    ]);
    Package {
        header: Header::parse(&header_text).unwrap(),
        header_text,
        actions: Vec::new(),
        grants: Vec::new(),
        bytes,
    }
}

/// One whole HTTP exchange: the status line and the body.
fn get(port: u16, request: &str) -> (String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(raw[..split].to_vec()).unwrap();
    (head, raw[split + 4..].to_vec())
}

#[test]
fn ranges_parse_as_pkg_asks_for_them() {
    assert_eq!(range("bytes=0-9", 100), Some((0, 10)));
    assert_eq!(range("bytes=90-", 100), Some((90, 100)));
    assert_eq!(range("bytes=50-999999", 100), Some((50, 100)));
    assert_eq!(range("bytes=99-99", 100), Some((99, 100)));
    for refused in [
        "bytes=100-",
        "bytes=10-5",
        "bytes=-5",
        "bytes=0-1,4-5",
        "bytes=+1-2",
        "items=0-9",
        "bytes=0",
        "bytes=18446744073709551615-18446744073709551615",
    ] {
        assert_eq!(range(refused, 100), None, "{refused}");
    }
}

#[test]
fn the_mirror_serves_files_whole_and_in_ranges() {
    let repo = tuf::build(&[notes()], &keys(1), NOW).unwrap();
    let timestamp = repo
        .files
        .iter()
        .find(|(p, _)| p == "metadata/timestamp.json")
        .unwrap()
        .1
        .clone();
    let (port, served) = mirror(repo);
    let ask = |request: &str| get(port, request);

    let (head, body) = ask("GET /metadata/timestamp.json HTTP/1.1\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert!(head.contains("Accept-Ranges: bytes"));
    assert_eq!(body, timestamp);

    let (head, body) = ask("GET /metadata/timestamp.json HTTP/1.1\r\nRange: bytes=2-5\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 206 Partial Content"), "{head}");
    assert!(head.contains(&format!("Content-Range: bytes 2-5/{}", timestamp.len())));
    assert_eq!(body, &timestamp[2..6]);
    assert_eq!(
        *served.lock().unwrap(),
        [("metadata/timestamp.json".to_string(), 2, 6)]
    );

    let (head, body) = ask("HEAD /metadata/timestamp.json HTTP/1.1\r\n\r\n");
    assert!(
        head.contains(&format!("Content-Length: {}", timestamp.len())),
        "{head}"
    );
    assert!(body.is_empty());

    let past = format!(
        "GET /metadata/timestamp.json HTTP/1.1\r\nRange: bytes={}-\r\n\r\n",
        timestamp.len()
    );
    let (head, _) = ask(&past);
    assert!(head.starts_with("HTTP/1.1 416"), "{head}");
    assert!(head.contains(&format!("Content-Range: bytes */{}", timestamp.len())));

    for (request, status) in [
        ("GET /metadata/2.root.json HTTP/1.1\r\n\r\n", "404"),
        ("GET /../Cargo.toml HTTP/1.1\r\n\r\n", "404"),
        ("GET metadata/timestamp.json HTTP/1.1\r\n\r\n", "404"),
        ("POST /metadata/timestamp.json HTTP/1.1\r\n\r\n", "405"),
        ("BREW\r\n\r\n", "400"),
    ] {
        let (head, _) = ask(request);
        assert!(
            head.starts_with(&format!("HTTP/1.1 {status}")),
            "{request:?}: {head}"
        );
    }
}

/// kaipkg's store over memory, as kaifs looks to it.
#[derive(Default)]
struct Memory(BTreeMap<String, Vec<u8>>);

impl Volume for Memory {
    fn begin(&mut self, _: u64) -> Result<(), String> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.get(path).cloned())
    }
    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        let prefix = format!("{dir}/");
        let mut names: Vec<String> = self
            .0
            .keys()
            .filter_map(|p| p.strip_prefix(&prefix))
            .map(|rest| rest.split('/').next().unwrap_or(rest).to_string())
            .collect();
        names.dedup();
        Ok(names)
    }
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.0.insert(path.into(), bytes.to_vec());
        Ok(())
    }
    fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.0
            .entry(path.into())
            .or_default()
            .extend_from_slice(bytes);
        Ok(())
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let bytes = self.0.remove(from).ok_or(format!("no {from}"))?;
        self.0.insert(to.into(), bytes);
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), String> {
        self.0.remove(path);
        Ok(())
    }
}

struct NoLedger;

impl Ledger for NoLedger {
    fn switched(
        &mut self,
        _: &mut dyn Volume,
        _: u64,
        _: &kaipkg::generation::Generation,
    ) -> Result<(), String> {
        Ok(())
    }
    fn collected(&mut self, _: &mut dyn Volume, _: &[u64], _: usize) -> Result<(), String> {
        Ok(())
    }
}

/// `pkg`'s side of a download, over the mirror: one `Range` GET per piece of
/// the object's consistent-snapshot file.
struct Http {
    port: u16,
}

impl Source for Http {
    fn fetch(&mut self, entry: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let path = format!(
            "targets/{}.{}-{}.kpkg",
            kaipkg::hex(&entry.sha),
            entry.name,
            entry.version
        );
        let last = offset + len as u64 - 1;
        let (head, body) = get(
            self.port,
            &format!("GET /{path} HTTP/1.1\r\nRange: bytes={offset}-{last}\r\n\r\n"),
        );
        match head.starts_with("HTTP/1.1 206") {
            true => Ok(body),
            false => Err(head),
        }
    }
}

/// The resolver's entries from the `packages` role's `custom.kai` fields (in
/// the OS, `kaituf` checks the role first).
fn entries(repo: &Repository) -> Vec<Entry> {
    let packages = &repo
        .files
        .iter()
        .find(|(p, _)| p == "metadata/1.packages.json")
        .unwrap()
        .1;
    let packages: Value = serde_json::from_slice(packages).unwrap();
    let targets = packages["signed"]["targets"].as_object().unwrap();
    targets
        .values()
        .map(|target| {
            let kai = &target["custom"]["kai"];
            let text = |v: &Value| v.as_str().unwrap().to_string();
            let sha: Sha = unhex(target["hashes"]["sha256"].as_str().unwrap()).unwrap();
            Entry {
                name: text(&kai["name"]),
                version: Version::parse(kai["version"].as_str().unwrap()).unwrap(),
                sha,
                size: target["length"].as_u64().unwrap(),
                depends: kai["depends"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| {
                        let (name, range) = d.as_str().unwrap().split_once(' ').unwrap();
                        Depends {
                            name: name.into(),
                            range: Range::parse(range).unwrap(),
                        }
                    })
                    .collect(),
                grants: kai["grants"].as_array().unwrap().iter().map(text).collect(),
                yanked: kai["yanked"].as_bool().unwrap(),
            }
        })
        .collect()
}

const OS: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];

/// Installs `names` from `mirror` into `vol`, after an empty generation 1.
fn install(vol: &mut Memory, port: u16, index: &[Entry], names: &[&str]) -> Result<u64, Error> {
    let mut ledger = NoLedger;
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: NOW,
        action: 1,
    };
    let store = Store::default();
    store.setup(&mut cx, Vec::new())?;
    let wants: Vec<Want<'_>> = names
        .iter()
        .map(|name| Want { name, range: None })
        .collect();
    store.install(&mut cx, &mut Http { port }, index, OS, &wants, false)
}

#[test]
fn the_store_installs_from_the_mirror_in_pieces() {
    let (notes, big) = (notes(), data(500 << 10));
    let repo = tuf::build(&[notes, big], &keys(1), NOW).unwrap();
    let index = entries(&repo);
    let (port, served) = mirror(repo);
    let mut vol = Memory::default();
    assert_eq!(
        install(&mut vol, port, &index, &["notes", "big-data"]),
        Ok(2)
    );
    for entry in &index {
        assert!(
            store::present(&mut vol, &entry.sha).unwrap(),
            "{}",
            entry.name
        );
    }
    let pieces = |name: &str| -> Vec<usize> {
        served
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _, _)| path.contains(name))
            .map(|(_, start, end)| end - start)
            .collect()
    };
    let big = index.iter().find(|e| e.name == "big-data").unwrap().size as usize;
    assert_eq!(pieces("big-data").len(), big.div_ceil(PIECE));
    assert!(pieces("big-data").iter().all(|&n| n <= PIECE));
    assert_eq!(pieces("big-data").iter().sum::<usize>(), big);
    assert_eq!(pieces("notes").len(), 1);
}

#[test]
fn a_mirror_serving_other_bytes_is_refused() {
    let repo = tuf::build(&[notes()], &keys(1), NOW).unwrap();
    let index = entries(&repo);
    let files = repo
        .files
        .into_iter()
        .map(|(path, mut bytes)| {
            if path.starts_with("targets/") {
                bytes[40] ^= 1;
            }
            (path, bytes)
        })
        .collect();
    let (port, _) = mirror(Repository { files });
    let mut vol = Memory::default();
    assert_eq!(
        install(&mut vol, port, &index, &["notes"]),
        Err(Error::Refused(Refused::BadHash("notes".into())))
    );
    assert!(!store::present(&mut vol, &index[0].sha).unwrap());
}
