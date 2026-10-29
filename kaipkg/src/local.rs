//! `local` (ADR 0011 §11, ADR 0012 D7; P4 task 13): the device's own
//! repository, for apps built in the OS with `cc build`. It is a TUF repository
//! like the image's ([`crate::sign`]), kept under [`DIR`] in the store's
//! volume, and one Ed25519 device key made at first use signs every role of it
//! ([`device_key`]). `pkg` checks it as it checks the image's, pinned to that
//! key ([`check`]), and installs from it into a new generation, so `undo` takes
//! a build back out.
//!
//! - **The key** never leaves `pkg`: its seed lies at [`KEY`], which only the
//!   holder of `/system/pkg` reads.
//! - **Names:** a build cannot take a name another repository offers or that
//!   is installed from one, so it never stands in for a signed package.
//! - **Versions:** `0.0.<n>`, one past the build it replaces; `local` holds the
//!   newest build of each name only.

use crate::bundle::{self, ARTIFACT, HEADER};
use crate::generation::Time;
use crate::header::{Header, valid_name};
use crate::image::{self, Checked, Files, ROOT};
use crate::mirror::SAVE_ACTION;
use crate::resolve::Entry;
use crate::sign::{self, Key, Keys, Periods, Target};
use crate::store::{Source, Volume};
use crate::version::Version;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{Value, json};

/// Where `local` lies in the store's volume.
pub const DIR: &str = "pkg/local";
/// The device key's 32-byte seed.
pub const KEY: &str = "pkg/local/key";
/// The world every app built in the OS is on (ADR 0012 D7).
pub const WORLD: &str = "kai:app@0.1";
/// Every build's summary: data, as any summary is.
pub const SUMMARY: &str = "Built in the OS with cc";
/// Its metadata comes from no network, and its check, the image's, reads no
/// expiry; the periods are there because TUF needs them.
const PERIODS: Periods = Periods {
    root: 3650,
    targets: 3650,
    snapshot: 3650,
    timestamp: 3650,
};

/// The device key: its seed from [`KEY`], or, at first use, from `entropy`,
/// kept there in one action.
pub fn device_key(
    vol: &mut dyn Volume,
    entropy: impl FnOnce() -> Result<[u8; 32], String>,
) -> Result<Key, String> {
    if let Some(seed) = vol.read(KEY)? {
        let seed: [u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| "the device key is not 32 bytes")?;
        return Ok(Key::from_seed(seed));
    }
    let seed = entropy()?;
    vol.begin(SAVE_ACTION)?;
    vol.write(KEY, &seed)?;
    vol.commit()?;
    Ok(Key::from_seed(seed))
}

/// A build put in `local`.
#[derive(Debug, PartialEq, Eq)]
pub struct Published {
    pub name: String,
    pub version: Version,
}

/// `artifact`, a component `cc build` wrote, as a package in `local`, signed by
/// `key`: named by its manifest, refused if `taken` holds that name, and
/// replacing an older build of it. The whole repository is written again in one
/// action, so a crash leaves the old one or the new.
pub fn publish(
    vol: &mut dyn Volume,
    key: &Key,
    artifact: &[u8],
    taken: &[&str],
    now: Time,
) -> Result<Published, String> {
    let manifest = livecore::artifact::parse(artifact)?.manifest;
    let name = manifest.component;
    valid_name(&name)?;
    if taken.contains(&name.as_str()) {
        return Err(format!(
            "{name} is another repository's package: a build cannot take its name"
        ));
    }
    let mut held = held(vol)?;
    let version = match held.iter().position(|(header, _, _)| header.name == name) {
        Some(at) => {
            let (old, _, _) = held.remove(at);
            Version::new(0, 0, old.version.patch + 1)
        }
        None => Version::new(0, 0, 1),
    };
    let text = format!(
        "kai-package 1\nname {name}\nversion {version}\nkind app\nsummary {SUMMARY}\n\
         licence {}\nworld {WORLD}\ndepends kai:app ^0.1\n",
        manifest.licence
    );
    let header = Header::parse(&text)?;
    let bytes = bundle::build(&[(HEADER, text.as_bytes()), (ARTIFACT, artifact)]);
    held.push((header, text, bytes));
    write(vol, key, &held, now)?;
    Ok(Published { name, version })
}

/// The builds `local` holds: each one's header, header text and bytes.
fn held(vol: &mut dyn Volume) -> Result<Vec<(Header, String, Vec<u8>)>, String> {
    let mut held = Vec::new();
    for file in vol.list(&format!("{DIR}/targets"))? {
        let bytes = vol
            .read(&format!("{DIR}/targets/{file}"))?
            .ok_or_else(|| format!("local's {file} went away"))?;
        let files = bundle::files(&bytes)?;
        let text = files
            .first()
            .filter(|(path, _)| *path == HEADER)
            .and_then(|(_, text)| core::str::from_utf8(text).ok())
            .ok_or_else(|| format!("local's {file} has no header"))?
            .to_string();
        held.push((Header::parse(&text)?, text, bytes));
    }
    Ok(held)
}

/// `local` as `held` says, signed by `key`, in one action: its metadata, each
/// build at `targets/<name>-<version>.kpkg` (the image's layout), and no other
/// target.
fn write(
    vol: &mut dyn Volume,
    key: &Key,
    held: &[(Header, String, Vec<u8>)],
    now: Time,
) -> Result<(), String> {
    let targets: Vec<Target> = held
        .iter()
        .map(|(header, text, bytes)| Target {
            path: format!("{}-{}.kpkg", header.name, header.version),
            bytes: bytes.clone(),
            kai: kai(header, text),
        })
        .collect();
    let files = sign::build(&targets, &Keys::one(key), now, PERIODS)?;
    let gone: Vec<String> = vol
        .list(&format!("{DIR}/targets"))?
        .into_iter()
        .filter(|file| !targets.iter().any(|t| t.path == *file))
        .collect();
    vol.begin(SAVE_ACTION)?;
    for file in gone {
        vol.remove(&format!("{DIR}/targets/{file}"))?;
    }
    for (path, bytes) in files {
        // The image's layout: a target's path without the SHA-256 before it.
        let path = match path.strip_prefix("targets/") {
            Some(rest) => format!("targets/{}", rest.split_once('.').map_or(rest, |(_, p)| p)),
            None => path,
        };
        vol.write(&format!("{DIR}/{path}"), &bytes)?;
    }
    vol.commit()
}

/// A build's `custom.kai` fields: its header's, no grants or actions of its own
/// (ADR 0012 D7), never yanked.
fn kai(header: &Header, text: &str) -> Value {
    let depends: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("depends "))
        .collect();
    json!({
        "name": header.name,
        "version": header.version.to_string(),
        "kind": header.kind.name(),
        "summary": header.summary,
        "licence": header.licence,
        "depends": depends,
        "grants": [],
        "actions": [],
        "yanked": false,
        "world": WORLD,
    })
}

/// `local` checked as the image's repository is ([`image::check`]), once its
/// root names `key` and no other: what it holds that verified, as a closure over
/// `provided`.
pub fn check(
    vol: &mut dyn Volume,
    key: &Key,
    provided: &[(&str, Version)],
) -> Result<Checked, String> {
    let root = vol
        .read(&format!("{DIR}/{ROOT}"))?
        .ok_or("local holds no build yet")?;
    let root: Value = serde_json::from_slice(&root).map_err(|_| "local's root is not JSON")?;
    let keys = root["signed"]["keys"]
        .as_object()
        .ok_or("local's root names no keys")?;
    if keys.len() != 1 || !keys.contains_key(&key.id()) {
        return Err("local's root is not the device key's".into());
    }
    image::check(&mut InVolume(vol), provided)
}

/// `local`'s files, from the store's volume.
struct InVolume<'a>(&'a mut dyn Volume);

impl Files for InVolume<'_> {
    fn file(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        match self.0.read(&format!("{DIR}/{path}"))? {
            Some(bytes) if bytes.len() as u64 > max => Err(format!("local's {path} is too long")),
            found => Ok(found),
        }
    }
}

/// The builds a [`check`] kept, as the store's source: each one by its SHA-256.
pub struct Held<'a>(pub &'a [(Entry, Vec<u8>)]);

impl Source for Held<'_> {
    fn fetch(&mut self, entry: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let (_, bytes) = self
            .0
            .iter()
            .find(|(held, _)| held.sha == entry.sha)
            .ok_or_else(|| format!("local does not hold {}", entry.name))?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        Ok(bytes[start..(start + len).min(bytes.len())].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::Generation;
    use crate::memvol::MemVolume;
    use crate::resolve::Want;
    use crate::store::{self, Ctx, Ledger, Store};
    use core::cell::Cell;
    use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION, SOURCE_SECTION};
    use livecore::artifact::{Manifest, append_custom_section};

    const NOW: Time = Time(1_793_000_000);
    const PROVIDED: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];

    /// A component `cc build` might write, named `name`, its source `source`.
    fn artifact(name: &str, source: &str) -> Vec<u8> {
        let mut module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
        let world = format!("app:{name}/{name}@0.0.1");
        let manifest = Manifest {
            lifecycle: 2,
            component: name.into(),
            version: "v1".into(),
            world: world.clone(),
            engine: "wasmi".into(),
            features: "-".into(),
            licence: "MIT".into(),
            build: "0123456789abcdef".into(),
            source: "fedcba9876543210".into(),
        };
        append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
        let source = livecore::artifact::bundle(&[("hello.c", source.as_bytes())]);
        append_custom_section(&mut module, SOURCE_SECTION, &source);
        let actions = format!("kai-actions 1\nworld {world}\n");
        append_custom_section(&mut module, ACTIONS_SECTION, actions.as_bytes());
        module
    }

    fn key(n: u8) -> Key {
        Key::from_seed([n; 32])
    }

    fn kept(checked: &Checked) -> Vec<String> {
        let names = checked.packages.iter();
        names
            .map(|(e, _)| format!("{} {}", e.name, e.version))
            .collect()
    }

    #[test]
    fn the_device_key_is_made_once_and_kept() {
        let mut vol = MemVolume::default();
        let made = Cell::new(0);
        let entropy = || {
            made.set(made.get() + 1);
            Ok([7; 32])
        };
        let first = device_key(&mut vol, entropy).unwrap();
        let again = device_key(&mut vol, || Err("asked twice".into())).unwrap();
        assert_eq!((first.id(), made.get()), (again.id(), 1));
        assert_eq!(first.id(), key(7).id());
        let mut short = MemVolume::default();
        short.write(KEY, &[1; 31]).unwrap();
        assert!(device_key(&mut short, || Ok([0; 32])).is_err());
    }

    #[test]
    fn a_build_is_published_signed_by_the_device_key_and_a_rebuild_replaces_it() {
        let mut vol = MemVolume::default();
        let key = key(1);
        let hello = artifact("hello", "int main(void) { return 0; }\n");
        let published = publish(&mut vol, &key, &hello, &["notes"], NOW).unwrap();
        assert_eq!(published.version, Version::new(0, 0, 1));
        let checked = check(&mut vol, &key, PROVIDED).unwrap();
        assert_eq!(kept(&checked), ["hello 0.0.1"]);
        assert!(checked.refused.is_empty(), "{:?}", checked.refused);
        assert_eq!(checked.indexes[0].summary, SUMMARY);
        let rebuilt = artifact("hello", "int main(void) { return 1; }\n");
        assert_eq!(
            publish(&mut vol, &key, &rebuilt, &[], NOW).unwrap().version,
            Version::new(0, 0, 2)
        );
        let other = artifact("other", "int x;\n");
        publish(&mut vol, &key, &other, &[], NOW).unwrap();
        let checked = check(&mut vol, &key, PROVIDED).unwrap();
        assert_eq!(kept(&checked), ["hello 0.0.2", "other 0.0.1"]);
        let targets = vol.list(&format!("{DIR}/targets")).unwrap();
        assert_eq!(targets, ["hello-0.0.2.kpkg", "other-0.0.1.kpkg"]);
    }

    #[test]
    fn a_build_cannot_take_another_repositorys_name_or_a_bad_one() {
        let mut vol = MemVolume::default();
        let notes = artifact("notes", "int main(void) { return 0; }\n");
        let err = publish(&mut vol, &key(1), &notes, &["notes"], NOW).unwrap_err();
        assert!(
            err.starts_with("notes is another repository's package"),
            "{err}"
        );
        let fs = artifact("fs", "int x;\n");
        assert!(
            publish(&mut vol, &key(1), &fs, &[], NOW).is_err(),
            "an OS row group"
        );
        assert!(publish(&mut vol, &key(1), b"not wasm", &[], NOW).is_err());
        assert!(vol.files().is_empty(), "nothing is written for a refusal");
    }

    #[test]
    fn local_is_pinned_to_the_device_key_and_a_flipped_byte_is_refused_by_name() {
        let mut vol = MemVolume::default();
        let hello = artifact("hello", "int main(void) { return 0; }\n");
        publish(&mut vol, &key(1), &hello, &[], NOW).unwrap();
        assert_eq!(
            check(&mut vol, &key(2), PROVIDED).err().as_deref(),
            Some("local's root is not the device key's")
        );
        let target = format!("{DIR}/targets/hello-0.0.1.kpkg");
        let mut bytes = vol.read(&target).unwrap().unwrap();
        let at = bytes.len() - 5;
        bytes[at] ^= 1;
        vol.write(&target, &bytes).unwrap();
        let checked = check(&mut vol, &key(1), PROVIDED).unwrap();
        assert!(checked.packages.is_empty());
        assert_eq!(checked.refused.len(), 1);
        assert!(
            checked.refused[0].1.starts_with("length or hash mismatch"),
            "{:?}",
            checked.refused
        );
    }

    struct NoLedger;

    impl Ledger for NoLedger {
        fn switched(&mut self, _: &mut dyn Volume, _: u64, _: &Generation) -> Result<(), String> {
            Ok(())
        }
        fn collected(&mut self, _: &mut dyn Volume, _: &[u64], _: usize) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_build_installs_from_local_as_a_generation_and_undo_takes_it_out() {
        let mut vol = MemVolume::default();
        let key = key(1);
        let hello = artifact("hello", "int main(void) { return 0; }\n");
        publish(&mut vol, &key, &hello, &[], NOW).unwrap();
        let checked = check(&mut vol, &key, PROVIDED).unwrap();
        let entries: Vec<Entry> = checked.packages.iter().map(|(e, _)| e.clone()).collect();
        let mut ledger = NoLedger;
        let mut cx = Ctx {
            vol: &mut vol,
            ledger: &mut ledger,
            now: NOW,
            action: 1,
        };
        let store = Store::default();
        store.setup(&mut cx, Vec::new()).unwrap();
        let want = [Want {
            name: "hello",
            range: None,
        }];
        let mut source = Held(&checked.packages);
        let n = store
            .install(&mut cx, &mut source, &entries, PROVIDED, &want, false)
            .unwrap();
        assert_eq!(n, 2);
        let names = |vol: &mut dyn Volume, n| -> Vec<String> {
            let generation = store::generation(vol, n).unwrap();
            generation.packages.into_iter().map(|p| p.name).collect()
        };
        assert_eq!(names(cx.vol, 2), ["hello"]);
        assert_eq!(store.rollback(&mut cx, 1), Ok(3));
        assert!(names(cx.vol, 3).is_empty());
    }
}
