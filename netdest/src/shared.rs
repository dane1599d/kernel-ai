//! Suffixes under which unrelated parties hold names (P5 question 3: no Public
//! Suffix List in v0.2, so a short list of our own). No grant or ceiling may
//! name one, since a host under it is anyone's: an app's `kai:net` grant
//! (`kainet::fetch`) and an agent package's `net` line (`kaiagent`) both refuse
//! them, from this one list.

/// Second-level public suffixes and shared hosting domains. Every top-level
/// domain is shared besides ([`is_shared`]).
pub const SHARED_SUFFIXES: &[&str] = &[
    "co.uk",
    "org.uk",
    "ac.uk",
    "gov.uk",
    "com.au",
    "net.au",
    "co.nz",
    "co.jp",
    "com.br",
    "co.in",
    "github.io",
    "githubusercontent.com",
    "gitlab.io",
    "pages.dev",
    "workers.dev",
    "netlify.app",
    "vercel.app",
    "web.app",
    "firebaseapp.com",
    "herokuapp.com",
    "appspot.com",
    "blogspot.com",
    "cloudfront.net",
    "azurewebsites.net",
    "s3.amazonaws.com",
    "fly.dev",
    // Parents under which every customer's names sit (C's review of #671).
    "amazonaws.com",
    "googleusercontent.com",
];

/// Whether `name` is a shared suffix: a top-level domain (one label) or one of
/// [`SHARED_SUFFIXES`], whatever its case and with one trailing dot or none. A
/// name under one (`me.github.io`) is not: it names one party's site.
pub fn is_shared(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    !name.contains('.')
        || SHARED_SUFFIXES
            .iter()
            .any(|shared| shared.eq_ignore_ascii_case(name))
}
