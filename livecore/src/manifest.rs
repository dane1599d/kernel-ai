//! A version's manifest, the `kai.manifest` section (ADR 0003, section 1).
//!
//! `kai-manifest 2` has these keys, in this order, each exactly once:
//!
//! ```text
//! kai-manifest 2
//! lifecycle 1
//! component notes
//! version 3
//! world app:notes/notes@1.0.0
//! engine wasmi
//! features -
//! licence MPL-2.0
//! build <sha256>
//! source <sha256>
//! ```
//!
//! `lifecycle` is the contract the module implements: `1` is today's hand-bound
//! `kai-lifecycle-1`, `2` is `kai:live` through the canonical ABI. `world` replaces version 1's `interface`: a swap keeps a
//! compatible world (the same package and world, and a version that links: the
//! same `0.minor` while 0.x, then the same major, never older). `kai-manifest 1` is
//! still read for one release, and its `interface` becomes the world.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::live::valid_label;

/// The lifecycle contracts this host can load: `kai-lifecycle-1` and, through
/// `appabi`, `kai:live`.
pub const LIFECYCLES_LOADED: [u32; 2] = [1, 2];
/// The lifecycle values a manifest may name.
const LIFECYCLES: [u32; 2] = [1, 2];
/// The engines this host has.
pub const ENGINES: &[&str] = &["wasmi"];
/// Longest licence expression.
pub const MAX_LICENCE: usize = 128;

/// What a version says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The lifecycle contract, 1 or 2.
    pub lifecycle: u32,
    pub component: String,
    pub version: String,
    /// The WIT world it implements, such as `app:notes/notes@1.0.0`; a swap keeps
    /// a compatible one. From a `kai-manifest 1`, its `interface`, such as `notes@1`.
    pub world: String,
    /// The engine that runs it.
    pub engine: String,
    /// Cargo features it was built with, `-` for none.
    pub features: String,
    /// Its licence as an SPDX expression; `-` when read from a `kai-manifest 1`.
    pub licence: String,
    /// SHA-256 of the module before the sections were added.
    pub build: String,
    /// SHA-256 of the source bundle.
    pub source: String,
}

const KEYS_1: [&str; 7] = [
    "lifecycle",
    "component",
    "version",
    "interface",
    "features",
    "build",
    "source",
];
const KEYS_2: [&str; 9] = [
    "lifecycle",
    "component",
    "version",
    "world",
    "engine",
    "features",
    "licence",
    "build",
    "source",
];

impl Manifest {
    /// The manifest as `kai-manifest 2` text.
    pub fn format(&self) -> String {
        format!(
            "kai-manifest 2\nlifecycle {}\ncomponent {}\nversion {}\nworld {}\nengine {}\nfeatures {}\nlicence {}\nbuild {}\nsource {}\n",
            self.lifecycle,
            self.component,
            self.version,
            self.world,
            self.engine,
            self.features,
            self.licence,
            self.build,
            self.source
        )
    }

    /// Reads a `kai-manifest 2`, or a `kai-manifest 1`: the header, then every key
    /// in order, once each.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let manifest = match lines.next() {
            Some("kai-manifest 2") => {
                let v = values(&mut lines, &KEYS_2)?;
                if World::parse(v[3]).is_none() {
                    return Err(format!(
                        "{:?} is not a world such as app:notes/notes@1.0.0",
                        v[3]
                    ));
                }
                Self {
                    lifecycle: lifecycle(v[0])?,
                    component: v[1].to_string(),
                    version: v[2].to_string(),
                    world: v[3].to_string(),
                    engine: v[4].to_string(),
                    features: v[5].to_string(),
                    licence: v[6].to_string(),
                    build: v[7].to_string(),
                    source: v[8].to_string(),
                }
            }
            Some("kai-manifest 1") => {
                let v = values(&mut lines, &KEYS_1)?;
                if v[0] != "1" {
                    return Err(format!("a kai-manifest 1 has lifecycle 1, not {}", v[0]));
                }
                Self {
                    lifecycle: 1,
                    component: v[1].to_string(),
                    version: v[2].to_string(),
                    world: world_of_interface(v[3])?,
                    engine: String::from("wasmi"),
                    features: v[4].to_string(),
                    licence: String::from("-"),
                    build: v[5].to_string(),
                    source: v[6].to_string(),
                }
            }
            _ => return Err("not a kai-manifest 1 or 2".into()),
        };
        if lines.next().is_some() {
            return Err("text after the source line".into());
        }
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&self) -> Result<(), String> {
        if !valid_label(&self.version) {
            return Err(format!("bad version label {:?}", self.version));
        }
        if !ENGINES.contains(&self.engine.as_str()) {
            return Err(format!("engine {} is not one of this host's", self.engine));
        }
        if self.licence.len() > MAX_LICENCE
            || !self
                .licence
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b" .+-()".contains(&b))
            || self.licence.starts_with(' ')
            || self.licence.ends_with(' ')
        {
            return Err(format!("bad licence {:?}", self.licence));
        }
        Ok(())
    }

    /// Why this host cannot load the version, if it cannot.
    pub fn unloadable(&self) -> Option<String> {
        (!LIFECYCLES_LOADED.contains(&self.lifecycle))
            .then(|| format!("lifecycle {} is not one this host loads", self.lifecycle))
    }
}

/// The value of each key in `keys`, in order. Values have no spaces, except the
/// licence, an SPDX expression such as `MIT OR Apache-2.0`.
fn values<'a>(lines: &mut core::str::Lines<'a>, keys: &[&str]) -> Result<Vec<&'a str>, String> {
    keys.iter()
        .map(|&key| {
            let line = lines.next().ok_or_else(|| format!("no {key} line"))?;
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '))
                .filter(|value| !value.is_empty() && (key == "licence" || !value.contains(' ')))
                .ok_or_else(|| format!("expected {key}, got {line:?}"))
        })
        .collect()
}

/// The world a `kai-manifest 1` interface means: `notes@1` is the world xtask
/// writes for notes now, `app:notes/notes@1.0.0`, so a version built before this
/// release still swaps live to one built after it. xtask never wrote anything else.
fn world_of_interface(interface: &str) -> Result<String, String> {
    match interface.split_once('@') {
        Some((name, major))
            if kebab(name) && !major.is_empty() && major.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Ok(format!("app:{name}/{name}@{major}.0.0"))
        }
        _ => Err(format!("interface {interface:?} is not <name>@<major>")),
    }
}

fn lifecycle(value: &str) -> Result<u32, String> {
    value
        .parse()
        .ok()
        .filter(|n| LIFECYCLES.contains(n))
        .ok_or_else(|| format!("lifecycle {value} is not 1 or 2"))
}

/// A world name: `ns:pkg/world@major.minor.patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct World<'a> {
    pub package: &'a str,
    pub world: &'a str,
    pub version: (u64, u64, u64),
}

impl<'a> World<'a> {
    /// Reads `ns:pkg/world@1.2.3`; names are kebab-case.
    pub fn parse(text: &'a str) -> Option<Self> {
        let (path, version) = text.split_once('@')?;
        let (package, world) = path.split_once('/')?;
        let (namespace, name) = package.split_once(':')?;
        if ![namespace, name, world].iter().all(|part| kebab(part)) {
            return None;
        }
        let mut numbers = version.split('.').map(|n| {
            (!n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()))
                .then(|| n.parse().ok())
                .flatten()
        });
        let version = (numbers.next()??, numbers.next()??, numbers.next()??);
        numbers.next().is_none().then_some(Self {
            package,
            world,
            version,
        })
    }

    /// The version it links at: `0.minor` while 0.x, then the major.
    fn line(&self) -> (u64, u64) {
        match self.version {
            (0, minor, _) => (0, minor),
            (major, ..) => (major, 0),
        }
    }
}

fn kebab(part: &str) -> bool {
    part.split('-').all(|word| {
        word.as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_lowercase())
            && word
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// Whether a candidate with world `candidate` may replace the active version's
/// `active` without a restart: the same world at a version that links and is not
/// older. Names that are not WIT worlds (a `kai-manifest 1`'s `notes@1`) must be
/// equal.
pub fn compatible(active: &str, candidate: &str) -> bool {
    match (World::parse(active), World::parse(candidate)) {
        (Some(a), Some(c)) => {
            a.package == c.package
                && a.world == c.world
                && a.line() == c.line()
                && c.version >= a.version
        }
        _ => active == candidate,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn manifest() -> Manifest {
        Manifest {
            lifecycle: 1,
            component: "notes".into(),
            version: "2".into(),
            world: "app:notes/notes@1.0.0".into(),
            engine: "wasmi".into(),
            features: "-".into(),
            licence: "MIT OR Apache-2.0".into(),
            build: "ab12".into(),
            source: "cd34".into(),
        }
    }

    #[test]
    fn a_manifest_round_trips_as_version_2() {
        let text = manifest().format();
        assert!(text.starts_with("kai-manifest 2\nlifecycle 1\n"));
        assert_eq!(Manifest::parse(&text), Ok(manifest()));
    }

    #[test]
    fn a_version_1_manifest_is_still_read() {
        let text = "kai-manifest 1\nlifecycle 1\ncomponent notes\nversion 2\ninterface notes@1\nfeatures -\nbuild ab12\nsource cd34\n";
        let read = Manifest::parse(text).unwrap();
        assert_eq!(read.world, "app:notes/notes@1.0.0");
        assert!(compatible(&read.world, &manifest().world));
        assert_eq!(
            (read.engine.as_str(), read.licence.as_str()),
            ("wasmi", "-")
        );
        assert!(Manifest::parse(&text.replace("lifecycle 1", "lifecycle 2")).is_err());
    }

    #[test]
    fn a_manifest_must_have_every_key_in_order_once() {
        let text = manifest().format();
        for bad in [
            text.replace("lifecycle 1", "lifecycle 3"),
            text.replace("lifecycle 1", "lifecycle one"),
            text.replace("version 2", "version V2"),
            text.replace("world app:notes/notes@1.0.0\n", ""),
            text.replace("world app:notes/notes@1.0.0", "world notes@1"),
            text.replace("engine wasmi", "engine wasmtime"),
            text.replace("features -", "features"),
            text.replace("features -", "features a b"),
            text.replace("licence MIT OR Apache-2.0", "licence MIT;rm"),
            text.replace("licence MIT OR Apache-2.0", "licence "),
            text.replace("kai-manifest 2", "kai-manifest 3"),
            text.replace("build ab12\nsource cd34", "source cd34\nbuild ab12"),
            text.clone() + "extra 1\n",
        ] {
            assert!(Manifest::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lifecycles_1_and_2_load_here() {
        assert_eq!(manifest().unloadable(), None);
        let two = Manifest {
            lifecycle: 2,
            ..manifest()
        };
        assert_eq!(two.unloadable(), None);
        let three = Manifest {
            lifecycle: 3,
            ..manifest()
        };
        assert!(three.unloadable().unwrap().contains("lifecycle 3"));
    }

    #[test]
    fn worlds_parse_and_compatible_versions_swap_live() {
        let world = World::parse("app:notes/notes@1.2.3").unwrap();
        assert_eq!((world.package, world.world), ("app:notes", "notes"));
        assert_eq!(world.version, (1, 2, 3));
        for bad in [
            "notes@1",
            "app:notes/notes@1.2",
            "app:Notes/notes@1.0.0",
            "app:notes/notes@1.0.0.0",
            "app:notes/-notes@1.0.0",
            "app:notes@1.0.0",
            "app:notes/notes@1.x.0",
        ] {
            assert_eq!(World::parse(bad), None, "{bad}");
        }
        assert!(compatible("app:notes/notes@1.0.0", "app:notes/notes@1.4.0"));
        assert!(!compatible(
            "app:notes/notes@1.4.0",
            "app:notes/notes@1.0.0"
        ));
        assert!(!compatible(
            "app:notes/notes@1.0.0",
            "app:notes/notes@2.0.0"
        ));
        assert!(compatible("kai:app/app@0.1.0", "kai:app/app@0.1.5"));
        assert!(!compatible("kai:app/app@0.1.0", "kai:app/app@0.2.0"));
        assert!(!compatible("app:notes/notes@1.0.0", "app:todo/notes@1.0.0"));
        assert!(compatible("odd", "odd"));
        assert!(!compatible("odd", "app:notes/notes@1.0.0"));
    }
}
