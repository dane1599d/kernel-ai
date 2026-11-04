//! The online repository (plan p5-networking, task 8, host half; ADR 0011
//! §4 and §8), behind feature `online`: TUF metadata from the update
//! repository's GitHub Pages host, and `.kpkg` files from its Release assets
//! (one release a package version, tagged `pkg-<name>-<version>`), through
//! GitHub's redirect to its release-assets host, over `kainet`'s HTTPS.
//!
//! [`Online`] is a [`Remote`] for `kaituf` and a [`Source`] for the store, as
//! [`crate::mirror::Mirror`] is for the fixed `pkgrepo=` mirror, so the same
//! checks run on what it fetches: metadata against the root the device holds,
//! a package by its length and SHA-256, then its header. The transport is
//! never trusted.
//!
//! Until `pkg` holds a `NetDest` for them (tasks 1 and 8's OS half), the
//! hosts are a fixed destination ([`Hosts`]): every request and every
//! redirect must be to one of them, on port 443, before it is made. `pkg`'s
//! wiring waits for TCP in ring 3 (task 2).

use crate::resolve::Entry;
use crate::store::Source;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kainet::http::{Connect, Fetch, HttpError, Limits, Method, Range, Url};
use kaituf::updater::Remote;

/// Where the repository is: its metadata's and its releases' URLs, and the
/// host GitHub redirects release downloads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hosts {
    /// The metadata directory's URL, ending in `/`.
    pub metadata: String,
    /// The releases' download URL, ending in `/`: a release asset is
    /// `<releases><tag>/<file>`.
    pub releases: String,
    /// The release-assets host.
    pub assets: String,
}

impl Hosts {
    /// `dane1599d/kernel-ai-updates` on GitHub (ADR 0011 §4).
    pub fn github() -> Self {
        Self {
            metadata: "https://dane1599d.github.io/kernel-ai-updates/metadata/".into(),
            releases: "https://github.com/dane1599d/kernel-ai-updates/releases/download/".into(),
            assets: "release-assets.githubusercontent.com".into(),
        }
    }

    /// Whether `host` on `port` is one of the repository's: the fixed
    /// destination `pkg` may reach.
    pub fn allows(&self, host: &str, port: u16) -> bool {
        let of = |url: &str| Url::parse(url).map(|u| u.host).unwrap_or_default();
        port == 443
            && (host == of(&self.metadata) || host == of(&self.releases) || host == self.assets)
    }

    /// Where `entry`'s package is: its release, `pkg-<name>-<version>`, and
    /// the file `<name>-<version>.kpkg`.
    pub fn package_url(&self, entry: &Entry) -> String {
        let file = format!("{}-{}", entry.name, entry.version);
        format!("{}pkg-{file}/{file}.kpkg", self.releases)
    }
}

/// The online repository through `C`.
pub struct Online<'a, C> {
    pub hosts: Hosts,
    pub connect: C,
    /// The caller's clock, in nanoseconds.
    pub clock: &'a dyn Fn() -> u64,
    /// How long one request may take, with its redirects.
    pub request_ns: u64,
    /// Requests made so far, redirects not counted.
    pub requests: usize,
}

impl<'a, C: Connect> Online<'a, C> {
    pub fn new(hosts: Hosts, connect: C, clock: &'a dyn Fn() -> u64, request_ns: u64) -> Self {
        Self {
            hosts,
            connect,
            clock,
            request_ns,
            requests: 0,
        }
    }

    /// `url`, or `range` of it, at most `max` bytes of body: its status and
    /// body (none unless 200 or 206).
    fn get(
        &mut self,
        url: &str,
        range: Option<Range>,
        max: u64,
    ) -> Result<(u16, Vec<u8>), HttpError> {
        let url = Url::parse(url).map_err(HttpError::Url)?;
        let hosts = &self.hosts;
        let allow = |host: &str, port: u16| hosts.allows(host, port);
        let fetch = Fetch {
            limits: Limits::new(max, (self.clock)().saturating_add(self.request_ns)),
            allow: &allow,
            clock: self.clock,
        };
        self.requests += 1;
        let mut body = Vec::new();
        let got = fetch.run(&mut self.connect, Method::Get, &url, range, &mut |bytes| {
            body.extend_from_slice(bytes);
            Ok(())
        })?;
        Ok((got.head.status, body))
    }
}

/// An HTTP failure in words, for `kaituf` and the store.
fn words(error: HttpError) -> String {
    format!("the repository: {error:?}")
}

impl<C: Connect> Remote for Online<'_, C> {
    fn fetch(&mut self, name: &str, max: u64) -> Result<Option<Vec<u8>>, kaituf::Error> {
        let url = format!("{}{name}", self.hosts.metadata);
        match self.get(&url, None, max) {
            Ok((200, body)) => Ok(Some(body)),
            Ok((403 | 404, _)) => Ok(None),
            Ok((status, _)) => Err(kaituf::Error::Io(format!(
                "the repository answered {status} for {name}"
            ))),
            Err(HttpError::TooLarge) => Err(kaituf::Error::TooLarge(name.to_string())),
            Err(error) => Err(kaituf::Error::Io(words(error))),
        }
    }
}

impl<C: Connect> Source for Online<'_, C> {
    /// Up to `len` bytes of `entry`'s package from `offset`, by `Range`:
    /// empty past its end (416). A release asset the repository does not
    /// serve is a fetch error, so the store keeps what it has of it.
    fn fetch(&mut self, entry: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let url = self.hosts.package_url(entry);
        let range = Range {
            first: offset,
            last: Some(offset + len as u64 - 1),
        };
        match self.get(&url, Some(range), len as u64).map_err(words)? {
            (206, body) => Ok(body),
            (416, _) => Ok(Vec::new()),
            (status, _) => Err(format!("the repository answered {status} for {url}")),
        }
    }
}
