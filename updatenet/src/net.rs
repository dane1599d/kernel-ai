//! The release's files over `kainet`'s HTTPS (ADR 0008 section 12, ADR 0011
//! section 8): the chunk list whole, and the image by `Range`. Every request and
//! every hop of a redirect must be to one of the repository's hosts on 443
//! (GitHub's release redirect to its release-assets host is allowed only because
//! [`Hosts`] names it), and is checked before it is made.
//!
//! Nothing here is trusted: what comes back is held to the signed metadata by the
//! caller (the list against its target, each chunk against the list).

use alloc::vec::Vec;
use kainet::chunks::CHUNK;
use kainet::download::Answer;
use kainet::http::{Connect, Fetch, HttpError, Limits, Method, Range, Url};
use kaipkg::online::{Hosts, Online};

/// The repository through a connection.
pub struct Over<'a, C> {
    pub hosts: &'a Hosts,
    pub connect: &'a mut C,
    /// The caller's clock, in nanoseconds.
    pub clock: &'a dyn Fn() -> u64,
    /// How long one request may take, with its redirects.
    pub request_ns: u64,
}

impl<'a, C: Connect> Over<'a, C> {
    /// Over the connection an [`Online`] repository holds, with its hosts and its
    /// clock: the metadata and the release are fetched alike.
    pub fn of(online: &'a mut Online<'_, C>) -> Self {
        Self {
            hosts: &online.hosts,
            connect: &mut online.connect,
            clock: online.clock,
            request_ns: online.request_ns,
        }
    }

    fn limits(&self, max: u64) -> Limits {
        Limits::new(max, (self.clock)().saturating_add(self.request_ns))
    }

    /// The small file at `url`, at most `max` bytes: its status, and its body
    /// (none unless 200).
    pub fn small(&mut self, url: &str, max: u64) -> Result<(u16, Vec<u8>), HttpError> {
        let url = Url::parse(url).map_err(HttpError::Url)?;
        let hosts = self.hosts;
        let allow = |host: &str, port: u16| hosts.allows(host, port);
        let fetch = Fetch {
            limits: self.limits(max),
            allow: &allow,
            clock: self.clock,
        };
        let mut body = Vec::new();
        let got = fetch.run(&mut *self.connect, Method::Get, &url, None, &mut |bytes| {
            body.extend_from_slice(bytes);
            Ok(())
        })?;
        Ok((got.head.status, body))
    }

    /// One `Range` of the image at `url`, its bytes passed to `sink`, and how the
    /// server answered: [`kainet::download::download`]'s fetcher.
    pub fn range(
        &mut self,
        url: &Url,
        range: Range,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<Answer, HttpError> {
        let hosts = self.hosts;
        let allow = |host: &str, port: u16| hosts.allows(host, port);
        let fetch = Fetch {
            limits: self.limits(CHUNK),
            allow: &allow,
            clock: self.clock,
        };
        let got = fetch.run(&mut *self.connect, Method::Get, url, Some(range), sink)?;
        Ok(Answer::from(&got))
    }
}
