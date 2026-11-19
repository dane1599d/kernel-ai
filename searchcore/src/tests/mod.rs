//! Fakes for the four OS parts: a home folder in memory with kaifs's generations,
//! an index file that a crash can tear, a model that counts its mappings, and a
//! caller's `Dir` that reads only some folders.

mod text;
mod walk;

use crate::{Access, Changed, Changes, Files, Models, Search, Store};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;
use semindex::{EmbedError, Embedder, EmbedderId, Scope};

/// The home folder: each file's object, bytes and the generation that last
/// changed it; and the objects gone since, with the generation they went at.
#[derive(Default)]
pub struct Home {
    files: BTreeMap<String, (u64, Vec<u8>, u64)>,
    gone: Vec<(u64, u64)>,
    generation: u64,
    next_object: u64,
}

impl Home {
    /// Writes `path` in a new generation: the object it had, or a new one.
    pub fn write(&mut self, path: &str, text: &str) -> u64 {
        self.generation += 1;
        let object = match self.files.get(path) {
            Some((object, ..)) => *object,
            None => {
                self.next_object += 1;
                self.next_object
            }
        };
        let entry = (object, text.as_bytes().to_vec(), self.generation);
        self.files.insert(path.to_string(), entry);
        object
    }

    pub fn delete(&mut self, path: &str) {
        self.generation += 1;
        if let Some((object, ..)) = self.files.remove(path) {
            self.gone.push((object, self.generation));
        }
    }

    /// Moves `from` to `to`, keeping its object, as a rename does.
    pub fn rename(&mut self, from: &str, to: &str) {
        self.generation += 1;
        if let Some((object, bytes, _)) = self.files.remove(from) {
            self.files
                .insert(to.to_string(), (object, bytes, self.generation));
        }
    }
}

fn stat(object: u64, size: u64, changed: u64) -> fsproto::Stat {
    fsproto::Stat {
        kind: fsproto::Kind::File,
        object,
        size,
        mtime: changed * 1_000,
        crtime: 0,
        changed,
    }
}

impl Files for Home {
    fn changes(&mut self, since: u64) -> Result<Changes, String> {
        let mut rows: Vec<Changed> = self
            .files
            .iter()
            .filter(|(_, (_, _, changed))| *changed > since)
            .map(|(path, (object, bytes, changed))| Changed {
                stat: stat(*object, bytes.len() as u64, *changed),
                path: Some(path.clone()),
            })
            .collect();
        rows.extend(
            self.gone
                .iter()
                .filter(|(_, changed)| *changed > since)
                .map(|(object, changed)| Changed {
                    stat: stat(*object, 0, *changed),
                    path: None,
                }),
        );
        // A folder's own change, which the walk passes over.
        rows.push(Changed {
            stat: fsproto::Stat {
                kind: fsproto::Kind::Dir,
                ..stat(999, 0, self.generation)
            },
            path: Some(String::from("/home/user")),
        });
        Ok(Changes {
            rows,
            generation: self.generation,
        })
    }

    fn read(&mut self, path: &str, max: u64) -> Result<Vec<u8>, String> {
        match self.files.get(path) {
            Some((_, bytes, _)) if bytes.len() as u64 <= max => Ok(bytes.clone()),
            Some(_) => Err(String::from("too large")),
            None => Err(String::from("not found")),
        }
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
