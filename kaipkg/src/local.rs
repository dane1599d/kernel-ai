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
//! - **Versions:** the artifact's own, from its manifest's world
//!   (`app:<name>/<world>@<version>`, as the packer reads one), newer than the
//!   build it replaces; [`next`] proposes `0.0.<n>` for `cc build` to write.
//!   `local` holds the newest build of each name only.
//! - **Only apps:** an app's artifact has a `kai.app` section naming it; a
//!   build on plain `kai:app/component` has none and is an artifact only, since
//!   ADR 0011 has no component package kind.
//! - **One version:** the manifest's label carries the world's version with
//!   dashes (`0-0-3`), so the two cannot disagree.

use crate::bundle::{self, ARTIFACT, HEADER};
use crate::generation::Time;
use crate::header::{Header, valid_name};
use crate::image::{self, Checked, Files, ROOT};
use crate::mirror::SAVE_ACTION;
use crate::resolve::{Entry, Want};
use crate::sign::{self, Key, Keys, Periods, Target};
use crate::store::{self, Ctx, Source, Store, Volume};
use crate::version::{Range, Version};
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
/// An app's ADR 0001 D9 section, which a plain component lacks.
pub const APP_SECTION: &str = "kai.app";
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

/// The version the next build of `name` takes unless the user names one:
/// `0.0.<n>`, one past the newest build `local` holds, or `0.0.1`.
pub fn next(vol: &mut dyn Volume, name: &str) -> Result<Version, String> {
    let held = held(vol)?;
    Ok(
        match held.iter().find(|(header, _, _)| header.name == name) {
            Some((old, _, _)) => Version::new(0, 0, old.version.patch + 1),
            None => Version::new(0, 0, 1),
        },
    )
}

/// `artifact`, an app `cc build` wrote, as a package in `local`, signed by
/// `key`: named and versioned by its manifest, refused if `taken` holds that
/// name or the version is not newer than the build it replaces. The whole
/// repository is written again in one action, so a crash leaves the old one or
/// the new.
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
    let version = manifest
        .world
        .strip_prefix(&format!("app:{name}/"))
        .and_then(|rest| rest.split_once('@'))
        .map(|(_, version)| version)
        .ok_or_else(|| format!("{name}'s world {} is not its own app's", manifest.world))?;
    let version = Version::parse(version).map_err(|why| format!("{name}'s version: {why}"))?;
    let label = format!("{}-{}-{}", version.major, version.minor, version.patch);
    if manifest.version != label {
        return Err(format!(
            "{name}'s manifest is version {}, not {label} as its world says",
            manifest.version
        ));
    }
    is_app(artifact, &name)?;
    let mut held = held(vol)?;
    if let Some(at) = held.iter().position(|(header, _, _)| header.name == name) {
        let old = held[at].0.version;
        if version <= old {
            return Err(format!(
                "{name} {version} is not newer than local's {name} {old}"
            ));
        }
        held.remove(at);
    }
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

/// Whether `artifact` is an app's: its `kai.app` section (ADR 0001 D9's
/// `kai-app 1`) names it `name`. A build on plain `kai:app/component` has none,
/// and is an artifact only.
fn is_app(artifact: &[u8], name: &str) -> Result<(), String> {
    let section = livecore::artifact::custom_section(artifact, APP_SECTION)?
        .ok_or_else(|| format!("{name} has no {APP_SECTION}: only apps install from local"))?;
    let text = core::str::from_utf8(section).map_err(|_| "its kai.app is not text")?;
    let mut lines = text.lines();
    let named = lines.next() == Some("kai-app 1") && lines.any(|l| l == format!("id {name}"));
    match named {
        true => Ok(()),
        false => Err(format!("its {APP_SECTION} is not a kai-app 1 for {name}")),
    }
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

/// The names of the builds `local` holds.
pub fn names(vol: &mut dyn Volume) -> Result<Vec<String>, String> {
    Ok(held(vol)?
        .into_iter()
        .map(|(header, _, _)| header.name)
        .collect())
}

/// `published` installed from `checked`, what [`check`] kept of `local`, into
/// a new generation: exactly its version, so a rebuild replaces the build
/// installed before it. The generation's number.
pub fn install(
    store: &Store,
    cx: &mut Ctx<'_>,
    checked: &Checked,
    published: &Published,
    provided: &[(&str, Version)],
) -> Result<u64, store::Error> {
    let entries: Vec<Entry> = checked.packages.iter().map(|(e, _)| e.clone()).collect();
    let exactly = Range::parse(&format!("={}", published.version))
        .map_err(|why| store::Error::Store(why.into()))?;
    let want = [Want {
        name: &published.name,
        range: Some(exactly),
    }];
    let mut source = Held(&checked.packages);
    store.install(cx, &mut source, &entries, provided, &want, false)
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
    use crate::store::Ledger;
    use core::cell::Cell;
    use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION, SOURCE_SECTION};
    use livecore::artifact::{Manifest, append_custom_section};

    const NOW: Time = Time(1_793_000_000);
    const PROVIDED: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];

    /// An app `cc build` might write: `name` at `version`, from `source`.
    fn artifact(name: &str, version: &str, source: &str) -> Vec<u8> {
        let mut module = component(name, version, source);
        let app = format!("kai-app 1\nid {name}\nabi 1.0\n");
        append_custom_section(&mut module, APP_SECTION, app.as_bytes());
        module
    }

    /// The same build on plain `kai:app/component`: no `kai.app`.
    fn component(name: &str, version: &str, source: &str) -> Vec<u8> {
        let mut module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
        let world = format!("app:{name}/{name}@{version}");
        let manifest = Manifest {
            lifecycle: 2,
            component: name.into(),
            version: version.replace('.', "-"),
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
        assert_eq!(next(&mut vol, "hello"), Ok(Version::new(0, 0, 1)));
        let hello = artifact("hello", "0.0.1", "int main(void) { return 0; }\n");
        let published = publish(&mut vol, &key, &hello, &["notes"], NOW).unwrap();
        assert_eq!(published.version, Version::new(0, 0, 1));
        let checked = check(&mut vol, &key, PROVIDED).unwrap();
        assert_eq!(kept(&checked), ["hello 0.0.1"]);
        assert!(checked.refused.is_empty(), "{:?}", checked.refused);
        assert_eq!(checked.indexes[0].summary, SUMMARY);
        assert_eq!(next(&mut vol, "hello"), Ok(Version::new(0, 0, 2)));
        // A rebuild must be newer than the build it replaces.
        let same = artifact("hello", "0.0.1", "int main(void) { return 2; }\n");
        assert_eq!(
            publish(&mut vol, &key, &same, &[], NOW).err().as_deref(),
            Some("hello 0.0.1 is not newer than local's hello 0.0.1")
        );
        let rebuilt = artifact("hello", "0.0.2", "int main(void) { return 1; }\n");
        assert_eq!(
            publish(&mut vol, &key, &rebuilt, &[], NOW).unwrap().version,
            Version::new(0, 0, 2)
        );
        // A user may name a version, under the same rule.
        let named = artifact("other", "1.2.0", "int x;\n");
        publish(&mut vol, &key, &named, &[], NOW).unwrap();
        let checked = check(&mut vol, &key, PROVIDED).unwrap();
        assert_eq!(kept(&checked), ["hello 0.0.2", "other 1.2.0"]);
        let targets = vol.list(&format!("{DIR}/targets")).unwrap();
        assert_eq!(targets, ["hello-0.0.2.kpkg", "other-1.2.0.kpkg"]);
    }

    #[test]
    fn a_plain_component_or_a_label_that_differs_from_the_world_is_refused() {
        let mut vol = MemVolume::default();
        let plain = component("hello", "0.0.3", "int x;\n");
        let err = publish(&mut vol, &key(1), &plain, &[], NOW).unwrap_err();
        assert!(err.ends_with("only apps install from local"), "{err}");
        let app = artifact("hello", "0.0.3", "int x;\n");
        // The label is the world's version with dashes.
        let label = b"0-0-3";
        let mut bent = app.clone();
        let at = bent.windows(label.len()).position(|w| w == label).unwrap();
        bent[at..at + label.len()].copy_from_slice(b"0-0-4");
        let err = publish(&mut vol, &key(1), &bent, &[], NOW).unwrap_err();
        assert!(err.contains("not 0-0-3 as its world says"), "{err}");
        assert!(vol.files().is_empty());
    }

    #[test]
    fn a_build_whose_world_is_another_apps_is_refused() {
        let mut vol = MemVolume::default();
        let mut other = artifact("hello", "0.0.1", "int x;\n");
        let world = b"app:hello/hello@0.0.1";
        let at = other.windows(world.len()).position(|w| w == world).unwrap();
        other[at..at + world.len()].copy_from_slice(b"app:notes/notes@0.0.1");
        let err = publish(&mut vol, &key(1), &other, &[], NOW).unwrap_err();
        assert!(err.ends_with("is not its own app's"), "{err}");
        assert!(vol.files().is_empty());
    }

    #[test]
    fn a_build_cannot_take_another_repositorys_name_or_a_bad_one() {
        let mut vol = MemVolume::default();
        let notes = artifact("notes", "0.0.1", "int main(void) { return 0; }\n");
        let err = publish(&mut vol, &key(1), &notes, &["notes"], NOW).unwrap_err();
        assert!(
            err.starts_with("notes is another repository's package"),
            "{err}"
        );
        let fs = artifact("fs", "0.0.1", "int x;\n");
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
        let hello = artifact("hello", "0.0.1", "int main(void) { return 0; }\n");
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
    fn a_build_installs_from_local_as_a_generation_and_a_rebuild_replaces_it() {
        let mut vol = MemVolume::default();
        let key = key(1);
        let mut ledger = NoLedger;
        let store = Store::default();
        let mut build = |vol: &mut MemVolume, version: &str, source: &str| -> u64 {
            let app = artifact("hello", version, source);
            let published = publish(vol, &key, &app, &[], NOW).unwrap();
            let checked = check(vol, &key, PROVIDED).unwrap();
            let mut cx = Ctx {
                vol,
                ledger: &mut ledger,
                now: NOW,
                action: 1,
            };
            install(&store, &mut cx, &checked, &published, PROVIDED).unwrap()
        };
        let mut cx = Ctx {
            vol: &mut vol,
            ledger: &mut NoLedger,
            now: NOW,
            action: 1,
        };
        store.setup(&mut cx, Vec::new()).unwrap();
        let installed = |vol: &mut dyn Volume, n| -> Vec<String> {
            let generation = store::generation(vol, n).unwrap();
            let packages = generation.packages.into_iter();
            packages
                .map(|p| format!("{} {}", p.name, p.version))
                .collect()
        };
        assert_eq!(build(&mut vol, "0.0.1", "int x;\n"), 2);
        assert_eq!(installed(&mut vol, 2), ["hello 0.0.1"]);
        assert_eq!(build(&mut vol, "0.0.2", "int y;\n"), 3);
        assert_eq!(installed(&mut vol, 3), ["hello 0.0.2"]);
        // Undo is the generation switch back: the build before, then none.
        let mut cx = Ctx {
            vol: &mut vol,
            ledger: &mut NoLedger,
            now: NOW,
            action: 1,
        };
        assert_eq!(store.rollback(&mut cx, 2), Ok(4));
        assert_eq!(installed(&mut vol, 4), ["hello 0.0.1"]);
        assert_eq!(names(&mut vol), Ok(alloc::vec![String::from("hello")]));
    }
}
