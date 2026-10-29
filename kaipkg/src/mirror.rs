//! Installs from a mirror (ADR 0011 §8; P4 task 2): the fixed `pkgrepo=`
//! destination, which serves the repository's layout, `metadata/` and
//! `targets/<sha256>.<name>-<version>.kpkg` (consistent snapshots).
//!
//! Everything goes through one [`Exchange`]: PKG_FETCH in the OS, a socket in
//! the host tests. Each request is `kabi::pkgrepo::request`, a GET of at most
//! [`PIECE`] bytes of one file, so each response stays under the kernel's 256
//! KiB limit; a longer file comes in pieces. The transport is never trusted:
//! metadata is checked with `kaituf` against the root the device holds
//! ([`TufCache`]), and a package by its length and SHA-256 when the store has
//! fetched it, then by its own header (`store`'s rule).

use crate::index::Index;
use crate::resolve::Entry;
use crate::store::{PIECE, Source, Volume};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kaituf::updater::{Cache, Remote, Updater};

/// The role that lists every package (ADR 0011 §3), delegated by `targets`.
pub const ROLE: &str = "packages";
/// Where the device keeps verified metadata in its store, by role.
pub const TUF_DIR: &str = "pkg/tuf";

/// One HTTP/1.1 request to the mirror and its whole response.
pub trait Exchange {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, String>;
}

/// The mirror, through `E`.
pub struct Mirror<E> {
    exchange: E,
    /// Requests sent so far.
    pub requests: usize,
}

impl<E: Exchange> Mirror<E> {
    pub fn new(exchange: E) -> Self {
        Mirror {
            exchange,
            requests: 0,
        }
    }

    /// Up to `len` bytes of `path` from `offset`: fewer at its end, none past
    /// it, and None when the mirror has no such file.
    pub fn range(
        &mut self,
        path: &str,
        offset: u64,
        len: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        let len = len.min(PIECE);
        if len == 0 {
            return Ok(Some(Vec::new()));
        }
        let request = kabi::pkgrepo::request(path, offset, offset + len as u64 - 1)?;
        self.requests += 1;
        let response = self.exchange.exchange(request.as_bytes())?;
        let response = llmproto::http::parse_response(&response, true)
            .map_err(|why| format!("the mirror's answer for {path}: {why:?}"))?;
        let body = match response.status {
            206 => response.body,
            // A whole file, from a server that ignores ranges.
            200 => {
                let start = usize::try_from(offset).unwrap_or(usize::MAX);
                response.body.get(start..).unwrap_or_default().to_vec()
            }
            416 => Vec::new(),
            403 | 404 => return Ok(None),
            status => return Err(format!("the mirror answered {status} for {path}")),
        };
        if body.len() > len && response.status == 206 {
            return Err(format!("the mirror sent more of {path} than was asked"));
        }
        Ok(Some(body.into_iter().take(len).collect()))
    }

    /// The whole of `path`, at most `max` bytes and one more to show it is too
    /// long; None when the mirror has no such file.
    pub fn whole(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let cap = usize::try_from(max).unwrap_or(usize::MAX).saturating_add(1);
        let mut out = Vec::new();
        loop {
            let want = PIECE.min(cap - out.len());
            let Some(piece) = self.range(path, out.len() as u64, want)? else {
                return match out.is_empty() {
                    true => Ok(None),
                    false => Err(format!("{path} went away while it was read")),
                };
            };
            let short = piece.len() < want;
            out.extend_from_slice(&piece);
            if short || out.len() >= cap {
                return Ok(Some(out));
            }
        }
    }
}

impl<E: Exchange> Remote for Mirror<E> {
    fn fetch(&mut self, name: &str, max: u64) -> Result<Option<Vec<u8>>, kaituf::Error> {
        self.whole(&format!("/metadata/{name}"), max)
            .map_err(kaituf::Error::Io)
    }
}

impl<E: Exchange> Source for Mirror<E> {
    fn fetch(&mut self, entry: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let path = format!(
            "/targets/{}.{}-{}.kpkg",
            crate::hex(&entry.sha),
            entry.name,
            entry.version
        );
        // A target the index lists but the mirror doesn't serve (403 or 404,
        // say while it syncs) is a fetch error, so the store keeps what it has
        // of it; an empty piece would end the object short, as a tampered one.
        self.range(&path, offset, len)?
            .ok_or_else(|| format!("the mirror has no {path}"))
    }
}

/// The device's trusted metadata, kept in its store under [`TUF_DIR`], one file
/// a role, as the repository served it. `root` starts as the root the image
/// carries.
pub struct TufCache<'a> {
    pub vol: &'a mut dyn Volume,
}

/// The action a save is (ADR 0004 §4): the cache's own, no logged action's
/// (ADR 0002), so undoing one never takes the cache back with it.
pub const SAVE_ACTION: u64 = 0;

impl Cache for TufCache<'_> {
    fn load(&mut self, role: &str) -> Result<Option<Vec<u8>>, kaituf::Error> {
        self.vol
            .read(&format!("{TUF_DIR}/{role}.json"))
            .map_err(kaituf::Error::Io)
    }

    /// One action, so one kaifs commit with its undo record: outside an
    /// action `fs` makes the truncate durable before the new bytes, and a
    /// crash between them would leave the root or the rollback floor empty.
    fn save(&mut self, role: &str, bytes: &[u8]) -> Result<(), kaituf::Error> {
        let path = format!("{TUF_DIR}/{role}.json");
        self.vol.begin(SAVE_ACTION).map_err(kaituf::Error::Io)?;
        self.vol.write(&path, bytes).map_err(kaituf::Error::Io)?;
        self.vol.commit().map_err(kaituf::Error::Io)
    }
}

/// What a mirror's index holds, and what of it was refused.
#[derive(Debug, Default)]
pub struct Listing {
    /// Each package's entry, in the order its role lists them.
    pub entries: Vec<Entry>,
    /// Each refused target's path, and why.
    pub refused: Vec<(String, String)>,
    /// Each listed package's index entry, for the catalogue (`crate::catalog`).
    pub indexes: Vec<Index>,
}

/// The mirror's packages at `now` (seconds since 1970 UTC): its metadata
/// refreshed from `cache`, which must hold a trusted root, then every target of
/// [`ROLE`] as an index entry. An error means the metadata did not verify, and
/// nothing is listed; a target whose entry does not parse is refused by name.
pub fn index<E: Exchange>(
    mirror: &mut Mirror<E>,
    cache: &mut dyn Cache,
    now: u64,
) -> Result<Listing, String> {
    let tuf = |e: kaituf::Error| e.to_string();
    let mut updater = Updater::new(mirror, cache, now).map_err(tuf)?;
    updater.refresh().map_err(tuf)?;
    let mut listing = Listing::default();
    for path in updater.listed(ROLE, "targets").map_err(tuf)? {
        let parsed = match updater.target(&path) {
            Ok(Some(target)) => Index::parse(&target).and_then(|index| {
                index.check_path(&target)?;
                Ok((index.entry()?, index))
            }),
            Ok(None) => Err("no trusted role names it".into()),
            Err(why) => Err(why.to_string()),
        };
        match parsed {
            Ok((entry, index)) => {
                listing.entries.push(entry);
                listing.indexes.push(index);
            }
            Err(why) => listing.refused.push((path, why)),
        }
    }
    Ok(listing)
}
