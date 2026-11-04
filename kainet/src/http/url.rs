//! The URLs P5 fetches: `https://<host>[:<port>]<path>`, nothing else. No
//! user or password, no fragment, no IP literal; a host is a DNS name.

use alloc::format;
use alloc::string::{String, ToString};

/// The longest host name (RFC 1035).
const MAX_HOST: usize = 253;
/// The longest path and query taken.
pub const MAX_PATH: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    /// Lower-cased.
    pub host: String,
    pub port: u16,
    /// From its `/`, with any query.
    pub path: String,
}

impl Url {
    /// `text` as a URL, or why not.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let rest = text
            .strip_prefix("https://")
            .ok_or("a URL starts with https://")?;
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, parse_port(port)?),
            None => (authority, 443),
        };
        check_host(host)?;
        check_path(path)?;
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
            path: path.into(),
        })
    }

    /// Where a `Location:` header leads from here: an absolute https URL,
    /// or a path on this host.
    pub fn join(&self, location: &str) -> Result<Self, &'static str> {
        if location.starts_with("https://") {
            return Self::parse(location);
        }
        if location.starts_with('/') && !location.starts_with("//") {
            check_path(location)?;
            return Ok(Self {
                path: location.into(),
                ..self.clone()
            });
        }
        Err("a redirect leads somewhere other than https or a path")
    }

    /// The `Host:` header's value.
    pub fn authority(&self) -> String {
        match self.port {
            443 => self.host.to_string(),
            port => format!("{}:{port}", self.host),
        }
    }
}

fn parse_port(port: &str) -> Result<u16, &'static str> {
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err("a URL's port is a number");
    }
    match port.parse::<u16>() {
        Ok(0) | Err(_) => Err("a URL's port is 1 to 65535"),
        Ok(port) => Ok(port),
    }
}

/// A DNS name: labels of letters, digits and hyphens, none empty, none longer
/// than 63, not starting or ending with a hyphen, and not all digits (so no
/// IP literal gets through).
fn check_host(host: &str) -> Result<(), &'static str> {
    if host.is_empty() || host.len() > MAX_HOST {
        return Err("a URL's host is 1 to 253 characters");
    }
    for label in host.split('.') {
        let good = !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !good {
            return Err("a URL's host is a DNS name");
        }
    }
    if host
        .rsplit('.')
        .next()
        .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("a URL's host is a name, not an address");
    }
    Ok(())
}

/// Visible ASCII from a `/`, with no fragment, at most [`MAX_PATH`].
fn check_path(path: &str) -> Result<(), &'static str> {
    let good = path.starts_with('/')
        && path.len() <= MAX_PATH
        && path.bytes().all(|b| b.is_ascii_graphic() && b != b'#');
    good.then_some(())
        .ok_or("a URL's path is visible ASCII, with no fragment")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_splits_into_host_port_and_path() {
        let url = Url::parse("https://HF.co/Qwen/m/resolve/abc/f.gguf?x=1").unwrap();
        assert_eq!(
            (url.host.as_str(), url.port, url.path.as_str()),
            ("hf.co", 443, "/Qwen/m/resolve/abc/f.gguf?x=1")
        );
        let bare = Url::parse("https://a.test").unwrap();
        assert_eq!(
            (bare.path.as_str(), bare.authority()),
            ("/", "a.test".into())
        );
        let port = Url::parse("https://a.test:8443/x").unwrap();
        assert_eq!((port.port, port.authority()), (8443, "a.test:8443".into()));
    }

    #[test]
    fn hostile_urls_are_refused() {
        for text in [
            "http://a.test/",
            "https://",
            "https:///x",
            "https://user@a.test/",
            "https://a.test:/",
            "https://a.test:0/",
            "https://a.test:65536/",
            "https://a.test:+80/",
            "https://10.0.2.2/",
            "https://[::1]/",
            "https://a..test/",
            "https://-a.test/",
            "https://a-.test/",
            "https://a_b.test/",
            "https://a.test/#frag",
            "https://a.test/a b",
            "https://a.test/\u{e9}",
            "https://a.test/\x00",
        ] {
            assert!(Url::parse(text).is_err(), "{text:?}");
        }
        let long = alloc::format!("https://{}.test/", "a".repeat(64));
        assert!(Url::parse(&long).is_err());
        let path = alloc::format!("https://a.test/{}", "a".repeat(MAX_PATH));
        assert!(Url::parse(&path).is_err());
    }

    #[test]
    fn a_location_joins_as_a_url_or_a_path_on_the_same_host() {
        let from = Url::parse("https://releases.test:8443/a").unwrap();
        assert_eq!(
            from.join("https://assets.test/b").unwrap(),
            Url::parse("https://assets.test/b").unwrap()
        );
        let same = from.join("/c?d").unwrap();
        assert_eq!(
            (same.host.as_str(), same.port, same.path.as_str()),
            ("releases.test", 8443, "/c?d")
        );
        for bad in ["//evil.test/x", "http://a.test/", "c", "", "javascript:x"] {
            assert!(from.join(bad).is_err(), "{bad:?}");
        }
    }
}
