//! Fakes for the four OS parts: a home folder in memory with kaifs's generations,
//! an index file that a crash can tear, a model that counts its mappings, and a
//! caller's `Dir` that reads only some folders.

mod moves;
mod text;
mod walk;

use crate::{Access, Changed, Changes, Files, Models, Search, Store};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;
use semindex::{EmbedError, Embedder, EmbedderId, Scope};

/// The home folder as kaifs keeps it: files and folders are objects, each with the
/// generation that last changed it. A rename keeps the object; moving a folder
/// changes the folder and the folders it left and joined, not the files under it;
/// a trashed or deleted object is gone from the home folder's view with no row of
/// its own, and only its folder's change says so (as `fs` answers since #1148).
#[derive(Clone)]
pub struct Home {
    files: BTreeMap<String, (u64, Vec<u8>, u64)>,
    dirs: BTreeMap<String, (u64, u64)>,
    generation: u64,
    next_object: u64,
    /// Files read, to show a move is not read again.
    pub reads: usize,
    /// Paths whose next read fails, once each: a read that fails now.
    pub fail_reads: Vec<String>,
    /// Paths whose every read fails.
    pub fail_always: Vec<String>,
    /// `fs`'s fallback count.
    pub fallbacks: u64,
}

const HOME: &str = "/home/user";

impl Default for Home {
    fn default() -> Self {
        let mut dirs = BTreeMap::new();
        dirs.insert(String::from(HOME), (1, 0));
        Self {
            files: BTreeMap::new(),
            dirs,
            generation: 0,
            next_object: 1,
            reads: 0,
            fail_reads: Vec::new(),
            fail_always: Vec::new(),
            fallbacks: 0,
        }
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or(HOME, |(parent, _)| parent)
}

fn under(path: &str, folder: &str) -> bool {
    path.strip_prefix(folder)
        .is_some_and(|rest| rest.starts_with('/'))
}

impl Home {
    fn object(&mut self) -> u64 {
        self.next_object += 1;
        self.next_object
    }

    /// Folder `path` changed in this generation; made, with its parents, if new.
    fn touch(&mut self, path: &str) {
        if !self.dirs.contains_key(path) {
            self.touch(parent(path));
            let object = self.object();
            self.dirs
                .insert(path.to_string(), (object, self.generation));
        }
        if let Some(dir) = self.dirs.get_mut(path) {
            dir.1 = self.generation;
        }
    }

    /// Writes `path` in a new generation: the object it had, or a new one.
    pub fn write(&mut self, path: &str, text: &str) -> u64 {
        self.generation += 1;
        let object = match self.files.get(path) {
            Some((object, ..)) => *object,
            None => {
                self.touch(parent(path));
                self.object()
            }
        };
        let entry = (object, text.as_bytes().to_vec(), self.generation);
        self.files.insert(path.to_string(), entry);
        object
    }

    pub fn delete(&mut self, path: &str) {
        self.generation += 1;
        if self.files.remove(path).is_some() {
            self.touch(parent(path));
        }
    }

    /// Moves file `from` to `to`, keeping its object, as a rename does.
    pub fn rename(&mut self, from: &str, to: &str) {
        self.generation += 1;
        if let Some((object, bytes, _)) = self.files.remove(from) {
            self.touch(parent(from));
            self.touch(parent(to));
            let entry = (object, bytes, self.generation);
            self.files.insert(to.to_string(), entry);
        }
    }

    /// Moves folder `from` to `to` with everything under it: the folder and the
    /// two it left and joined change, and the files under it keep their objects
    /// and their generations.
    pub fn rename_folder(&mut self, from: &str, to: &str) {
        self.generation += 1;
        let moved = |path: &str| format!("{to}{}", &path[from.len()..]);
        let files: Vec<String> = self
            .files
            .keys()
            .filter(|p| under(p, from))
            .cloned()
            .collect();
        for path in files {
            let entry = self.files.remove(&path).unwrap();
            self.files.insert(moved(&path), entry);
        }
        let dirs: Vec<String> = self
            .dirs
            .keys()
            .filter(|p| *p == from || under(p, from))
            .cloned()
            .collect();
        for path in dirs {
            let entry = self.dirs.remove(&path).unwrap();
            self.dirs.insert(moved(&path), entry);
        }
        self.touch(parent(from));
        self.touch(parent(to));
        self.touch(to);
    }

    /// `earlier`'s files, folders and generation, with the fall-back count moved
    /// on: a fall back mount that undid the commits made since.
    pub fn fall_back_to(&mut self, earlier: &Home) {
        self.files = earlier.files.clone();
        self.dirs = earlier.dirs.clone();
        self.generation = earlier.generation;
        self.fallbacks += 1;
    }

    /// Removes `path` with no change anywhere: what a fallback may have missed.
    pub fn delete_unseen(&mut self, path: &str) {
        self.files.remove(path);
    }

    /// Trashes folder `path` with everything under it: only its folder changes.
    pub fn trash_folder(&mut self, path: &str) {
        self.generation += 1;
        self.files.retain(|p, _| !under(p, path));
        self.dirs.retain(|p, _| p != path && !under(p, path));
        self.touch(parent(path));
    }
}

fn stat(kind: fsproto::Kind, object: u64, size: u64, changed: u64) -> fsproto::Stat {
    fsproto::Stat {
        kind,
        object,
        size,
        mtime: changed * 1_000,
        crtime: 0,
        changed,
    }
}

impl Home {
    fn file_rows(&self, keep: impl Fn(&str, u64) -> bool) -> Vec<Changed> {
        self.files
            .iter()
            .filter(|(path, (_, _, changed))| keep(path, *changed))
            .map(|(path, (object, bytes, changed))| Changed {
                stat: stat(fsproto::Kind::File, *object, bytes.len() as u64, *changed),
                path: Some(path.clone()),
            })
            .collect()
    }
}

impl Files for Home {
    fn changes(&mut self, since: u64) -> Result<Changes, String> {
        let mut rows = self.file_rows(|_, changed| changed > since);
        rows.extend(
            self.dirs
                .iter()
                .filter(|(_, (_, changed))| *changed > since)
                .map(|(path, (object, changed))| Changed {
                    stat: stat(fsproto::Kind::Dir, *object, 0, *changed),
                    path: Some(path.clone()),
                }),
        );
        Ok(Changes {
            rows,
            generation: self.generation,
            fallbacks: self.fallbacks,
        })
    }

    fn read(&mut self, path: &str, max: u64) -> Result<Vec<u8>, String> {
        self.reads += 1;
        if self.fail_always.iter().any(|p| p == path) {
            return Err(String::from("broken"));
        }
        if let Some(at) = self.fail_reads.iter().position(|p| p == path) {
            self.fail_reads.remove(at);
            return Err(String::from("busy"));
        }
        match self.files.get(path) {
            Some((_, bytes, _)) if bytes.len() as u64 <= max => Ok(bytes.clone()),
            Some(_) => Err(String::from("too large")),
            None => Err(String::from("not found")),
        }
    }

    fn files_under(&mut self, folder: &str) -> Result<Vec<Changed>, String> {
        let mut rows = self.file_rows(|path, _| under(path, folder));
        rows.extend(
            self.dirs
                .iter()
                .filter(|(path, _)| under(path, folder))
                .map(|(path, (object, changed))| Changed {
                    stat: stat(fsproto::Kind::Dir, *object, 0, *changed),
                    path: Some(path.clone()),
                }),
        );
        Ok(rows)
    }

    fn present(&mut self, path: &str) -> Result<bool, String> {
        Ok(self.files.contains_key(path) || self.dirs.contains_key(path))
    }
}

/// The index file. `tear: Some((n, kept))` keeps only `kept` bytes of the `n`th
/// append and fails it, as a crash in the middle of a transaction would if kaifs
/// did not hold.
#[derive(Clone, Default)]
pub struct Disk {
    pub bytes: Vec<u8>,
    pub appends: usize,
    pub tear: Option<(usize, usize)>,
}

impl Store for Disk {
    fn load(&mut self) -> Result<Vec<u8>, String> {
        Ok(self.bytes.clone())
    }

    fn truncate(&mut self, len: usize) -> Result<(), String> {
        self.bytes.truncate(len);
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.appends += 1;
        match self.tear {
            Some((at, kept)) if at == self.appends => {
                self.tear = None;
                self.bytes
                    .extend_from_slice(&bytes[..kept.min(bytes.len())]);
                Err(String::from("the power went"))
            }
            _ => {
                self.bytes.extend_from_slice(bytes);
                Ok(())
            }
        }
    }

    fn replace(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.bytes = bytes.to_vec();
        Ok(())
    }
}

/// Words of four topics; a vector counts each topic's words.
const TOPICS: [&[&str]; 4] = [
    &["hotel", "trip", "flight", "lisbon", "travel"],
    &["invoice", "tax", "bank", "budget"],
    &["recipe", "soup", "bread"],
    &["meeting", "agenda", "notes"],
];

pub fn topics_id() -> EmbedderId {
    EmbedderId {
        name: String::from("topics-test"),
        sha256: [7; 32],
        dims: 4,
    }
}

/// The model, mapped: it says when it is unmapped.
pub struct Topics {
    live: Rc<Cell<i32>>,
}

impl Drop for Topics {
    fn drop(&mut self) {
        self.live.set(self.live.get() - 1);
    }
}

fn topics(text: &str, out: &mut [f32]) {
    let lower = text.to_lowercase();
    for (value, topic) in out.iter_mut().zip(TOPICS) {
        *value = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| topic.contains(w))
            .count() as f32;
    }
}

impl Embedder for Topics {
    fn id(&self) -> EmbedderId {
        topics_id()
    }

    fn embed_document(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError> {
        topics(text, out);
        Ok(())
    }

    fn embed_query(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError> {
        topics(text, out);
        Ok(())
    }
}

/// The model's pin and the machine's memory.
pub struct Model {
    pub id: Option<EmbedderId>,
    pub room: bool,
    pub loads: Rc<Cell<u32>>,
    pub live: Rc<Cell<i32>>,
}

impl Model {
    pub fn keyword() -> Self {
        Self {
            id: None,
            ..Self::topics()
        }
    }

    pub fn topics() -> Self {
        Self {
            id: Some(topics_id()),
            room: true,
            loads: Rc::default(),
            live: Rc::default(),
        }
    }
}

impl Models for Model {
    fn id(&self) -> Option<EmbedderId> {
        self.id.clone()
    }

    fn room(&self) -> bool {
        self.room
    }

    fn load(&mut self) -> Result<Box<dyn Embedder>, String> {
        self.loads.set(self.loads.get() + 1);
        self.live.set(self.live.get() + 1);
        Ok(Box::new(Topics {
            live: self.live.clone(),
        }))
    }
}

/// A caller's `Dir`: it reads under these folders only.
pub struct Dir(pub Vec<&'static str>);

impl Access for Dir {
    fn may_read(&mut self, path: &str) -> bool {
        self.0
            .iter()
            .any(|folder| semindex::scope::within(path, folder))
    }
}

pub fn everything() -> Dir {
    Dir(alloc::vec!["/home/user"])
}

pub type Service = Search<Home, Disk, Model>;

pub fn open(home: Home, disk: Disk, model: Model) -> (Service, crate::Opened) {
    Search::open(home, disk, model, Scope::new()).unwrap()
}

/// The paths a reply names, best first.
pub fn paths(reply: &str) -> Vec<String> {
    match crate::text::Reply::parse(reply) {
        Some(crate::text::Reply::Hits(hits)) => hits.into_iter().map(|hit| hit.path).collect(),
        other => panic!("{reply:?} read as {other:?}"),
    }
}

pub fn ask(query: &str) -> String {
    crate::text::Request {
        query: query.to_string(),
        since_ns: None,
        folder: None,
        limit: None,
    }
    .encode()
}
