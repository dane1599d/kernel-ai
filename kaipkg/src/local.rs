//! `local` (ADR 0011 §11, ADR 0012 D7; P4 task 13): the device's own
//! repository, for apps built in the OS with `cc build`. It is a TUF repository
//! like the image's ([`crate::sign`]), kept under [`DIR`] in the store's
//! volume, and one Ed25519 device key made at first use signs every role of it
//! ([`device_key`]). `pkg` checks it as it checks the image's, pinned to that
//! key ([`check`]), and installs from it into a new generation, so `undo` takes
//! a build back out.
//!
//! - **The key** never leaves `pkg`: its seed lies at [`KEY`], which only the
//!   holder of `/system/pkg` reads, and nothing prints it.
//! - **The pin:** `local`'s root must hold the device key itself, not just its
//!   id, and that same root is the one checked.
//! - **Only what verified is signed again:** a publish carries forward the
//!   builds the current metadata verifies, never a file merely lying there.
//! - **Names:** a build cannot take a name another repository offers or that
//!   is installed from one (the caller says which), so it never stands in for
//!   a signed package.
//! - **Versions:** the artifact's own, from its manifest's world
//!   (`app:<name>/<world>@<version>`, as the packer reads one), newer than the
//!   build it replaces; [`next`] proposes one. The manifest's label carries the
//!   same version with dashes (`0-0-3`), so the two cannot disagree. `local`
//!   holds the newest build of each name only.
//! - **Only apps that ask for nothing:** an app's artifact has a `kai.app`
//!   naming it (ADR 0001 D9) with no `use` or `expose`, and a `kai.actions` with
//!   no rows and no `effects` (ADR 0012 D7). A build on plain `kai:app/component`
//!   has no `kai.app` and is an artifact only. Nothing a build could call goes
//!   unshown: an app that asks for more needs a card first, which v0.1 lacks.
//! - **Bounds:** at most [`MAX_BUILDS`] builds and [`MAX_BYTES`] in all, so a
//!   publish fits `pkg`'s heap.

use crate::bundle::{self, ARTIFACT, HEADER};
use crate::generation::Time;
use crate::header::{Header, valid_name};
use crate::image::{self, Checked, Files, ROOT};
use crate::mirror::SAVE_ACTION;
use crate::resolve::{Entry, Want};
use crate::sign::{self, Key, Keys, Periods, Target};
use crate::store::{self, Ctx, Source, Store, Volume};
use crate::version::{Range, Version};
use crate::{Sha, sha256};
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
/// Most builds `local` holds.
pub const MAX_BUILDS: usize = 8;
/// Most bytes its builds hold in all: a publish holds them several times over
/// while it signs, within `pkg`'s heap.
pub const MAX_BYTES: usize = 1 << 20;
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
    if let Some(key) = saved_key(vol)? {
        return Ok(key);
    }
    let seed = entropy()?;
    store::in_action(vol, SAVE_ACTION, |vol| vol.write(KEY, &seed))?;
    Ok(Key::from_seed(seed))
}

/// The device key if one was made, read and never made: a question such as
/// [`next`] writes nothing.
pub fn saved_key(vol: &mut dyn Volume) -> Result<Option<Key>, String> {
    let Some(seed) = vol.read(KEY)? else {
        return Ok(None);
    };
    let seed: [u8; 32] = seed
        .as_slice()
        .try_into()
        .map_err(|_| "the device key is not 32 bytes")?;
    Ok(Some(Key::from_seed(seed)))
}

/// A build put in `local`.
#[derive(Debug, PartialEq, Eq)]
pub struct Published {
    pub name: String,
    pub version: Version,
}

/// One build `local` holds that verified: its header, header text and bytes.
type Build = (Header, String, Vec<u8>);

/// The version the next build of `name` takes unless the user names one: one
/// patch past the newest build `local` holds, or `0.0.1`. `key` is the device
/// key ([`saved_key`]); with none, `local` holds nothing yet.
pub fn next(
    vol: &mut dyn Volume,
    key: Option<&Key>,
    provided: &[(&str, Version)],
    name: &str,
) -> Result<Version, String> {
    let held = match key {
        Some(key) => verified(vol, key, provided)?,
        None => Vec::new(),
    };
    Ok(
        match held.iter().find(|(header, _, _)| header.name == name) {
            Some((old, _, _)) => {
                Version::new(old.version.major, old.version.minor, old.version.patch + 1)
            }
            None => Version::new(0, 0, 1),
        },
    )
}

/// `artifact`, an app `cc build` wrote, as a package in `local`, signed by
/// `key`: named and versioned by its manifest, refused if `taken` holds that
/// name, the version is not newer than the build it replaces, or it asks for
/// anything. The builds that verified are written again with it, in one action,
/// so a crash leaves the old repository or the new.
pub fn publish(
    vol: &mut dyn Volume,
    key: &Key,
    artifact: &[u8],
    taken: &[&str],
    provided: &[(&str, Version)],
    now: Time,
) -> Result<Published, String> {
    let parsed = livecore::artifact::parse(artifact)?;
    let manifest = parsed.manifest;
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
    declares_nothing(parsed.actions, &name)?;
    let mut held = verified(vol, key, provided)?;
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
    let total: usize = held.iter().map(|(_, _, bytes)| bytes.len()).sum();
    if held.len() > MAX_BUILDS || total > MAX_BYTES {
        return Err(format!(
            "local holds at most {MAX_BUILDS} builds and {} KiB: remove one first",
            MAX_BYTES >> 10
        ));
    }
    write(vol, key, &held, now)?;
    Ok(Published { name, version })
}

/// Whether `artifact` is an app's that asks for nothing: its `kai.app` section
/// (ADR 0001 D9's `kai-app 1`) names it `name` once and holds only `id` and
/// `abi` lines, no `use` or `expose`. A build on plain `kai:app/component` has
/// none, and is an artifact only.
fn is_app(artifact: &[u8], name: &str) -> Result<(), String> {
    let section = livecore::artifact::custom_section(artifact, APP_SECTION)?
        .ok_or_else(|| format!("{name} has no {APP_SECTION}: only apps install from local"))?;
    let text = core::str::from_utf8(section).map_err(|_| "its kai.app is not text")?;
    let mut lines = text.lines();
    let wrong = || format!("its {APP_SECTION} is not a kai-app 1 for {name} that asks for nothing");
    if lines.next() != Some("kai-app 1") {
        return Err(wrong());
    }
    let mut ids = 0;
    for line in lines {
        match line.split_once(' ') {
            Some(("id", id)) if id == name => ids += 1,
            Some(("abi", _)) => {}
            _ => return Err(wrong()),
        }
    }
    match ids {
        1 => Ok(()),
        _ => Err(wrong()),
    }
}

/// Whether the artifact's `kai.actions` declares no action rows and no
/// `effects` line: an app built in the OS has an empty `actions` (ADR 0012 D7),
/// and any row its effects could call would need showing first.
fn declares_nothing(actions: Option<&str>, name: &str) -> Result<(), String> {
    let Some(text) = actions else {
        return Ok(());
    };
    if text.lines().any(|line| line.starts_with("effects")) {
        return Err(format!(
            "{name} declares effects: an app built in the OS declares none"
        ));
    }
    let rows = livecore::actions::parse(text)?.rows;
    match rows.is_empty() {
        true => Ok(()),
        false => Err(format!(
            "{name} declares actions: an app built in the OS has an empty actions"
        )),
    }
}

/// The builds `local`'s metadata verifies, pinned to `key` ([`check`]); none
/// before the first publish. Metadata that does not verify is an error, so a
/// publish never signs again what it could not check.
fn verified(
    vol: &mut dyn Volume,
    key: &Key,
    provided: &[(&str, Version)],
) -> Result<Vec<Build>, String> {
    if vol.read(&format!("{DIR}/{ROOT}"))?.is_none() {
        return Ok(Vec::new());
    }
    let checked =
        check(vol, key, provided).map_err(|why| format!("local does not verify: {why}"))?;
    let mut held = Vec::new();
    for (_, bytes) in checked.packages {
        let files = bundle::files(&bytes)?;
        let text = files
            .first()
            .filter(|(path, _)| *path == HEADER)
            .and_then(|(_, text)| core::str::from_utf8(text).ok())
            .ok_or("a build has no header")?
            .to_string();
        held.push((Header::parse(&text)?, text, bytes));
    }
    Ok(held)
}

/// `local` as `held` says, signed by `key`, in one action: its metadata, each
/// build at `targets/<name>-<version>.kpkg` (the image's layout), and no other
/// target.
fn write(vol: &mut dyn Volume, key: &Key, held: &[Build], now: Time) -> Result<(), String> {
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
    store::in_action(vol, SAVE_ACTION, |vol| {
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
        Ok(())
    })
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
/// root holds `key` itself and no other key: what it holds that verified, as a
/// closure over `provided`. The root pinned is the one checked.
pub fn check(
    vol: &mut dyn Volume,
    key: &Key,
    provided: &[(&str, Version)],
) -> Result<Checked, String> {
    let root = vol
        .read(&format!("{DIR}/{ROOT}"))?
        .ok_or("local holds no build yet")?;
    let parsed: Value = serde_json::from_slice(&root).map_err(|_| "local's root is not JSON")?;
    let keys = parsed["signed"]["keys"]
        .as_object()
        .ok_or("local's root names no keys")?;
    // The key itself, not its id: kaituf takes a key's id as the root writes it.
    if keys.len() != 1 || keys.get(&key.id()) != Some(&key.public()) {
        return Err("local's root is not the device key's".into());
    }
    image::check(&mut Pinned { vol, root }, provided)
}

/// The SHA-256 of each build `local`'s metadata verifies: the store objects
/// `local` built, and no others.
pub fn shas(
    vol: &mut dyn Volume,
    key: &Key,
    provided: &[(&str, Version)],
) -> Result<Vec<Sha>, String> {
    let held = verified(vol, key, provided)?;
    Ok(held.iter().map(|(_, _, bytes)| sha256(bytes)).collect())
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

/// `local`'s files, from the store's volume, its root the one [`check`] pinned.
struct Pinned<'a> {
    vol: &'a mut dyn Volume,
    root: Vec<u8>,
}

impl Files for Pinned<'_> {
    fn file(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let found = match path == ROOT {
            true => Some(self.root.clone()),
            false => self.vol.read(&format!("{DIR}/{path}"))?,
        };
        match found {
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
        asking(name, version, source, "", "")
    }

    /// [`artifact`] with `more` after its `kai.actions` world line and `app`
    /// after its `kai.app` id line.
    fn asking(name: &str, version: &str, source: &str, more: &str, app: &str) -> Vec<u8> {
        let mut module = built(name, version, source, more);
        let app = format!("kai-app 1\nid {name}\nabi 1.0\n{app}");
        append_custom_section(&mut module, APP_SECTION, app.as_bytes());
        module
    }

    /// The same build on plain `kai:app/component`: no `kai.app`.
    fn component(name: &str, version: &str, source: &str) -> Vec<u8> {
        built(name, version, source, "")
    }

    fn built(name: &str, version: &str, source: &str, more: &str) -> Vec<u8> {
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
        let actions = format!("kai-actions 1\nworld {world}\n{more}");
        append_custom_section(&mut module, ACTIONS_SECTION, actions.as_bytes());
        module
    }

    #[test]
    fn the_pin_is_the_device_keys_own_material_not_just_its_id() {
        let mut vol = MemVolume::default();
        let hello = artifact("hello", "0.0.1", "int x;\n");
        publish(&mut vol, &key(1), &hello, &[], PROVIDED, NOW).unwrap();
        // The device key's id, holding another key: kaituf would take it.
        let path = format!("{DIR}/{ROOT}");
        let mut root: Value = serde_json::from_slice(&vol.read(&path).unwrap().unwrap()).unwrap();
        root["signed"]["keys"][key(1).id()] = key(2).public();
        vol.write(&path, &serde_json::to_vec(&root).unwrap())
            .unwrap();
        assert_eq!(
            check(&mut vol, &key(1), PROVIDED).err().as_deref(),
            Some("local's root is not the device key's")
        );
    }

    #[test]
    fn a_file_planted_in_local_is_never_signed_again() {
        let mut vol = MemVolume::default();
        let key = key(1);
        let hello = artifact("hello", "0.0.1", "int x;\n");
        publish(&mut vol, &key, &hello, &[], PROVIDED, NOW).unwrap();
        // A build lying in local's targets that its metadata does not list.
        let mut other = MemVolume::default();
        let evil = artifact("evil", "0.0.1", "int evil;\n");
        publish(&mut other, &key, &evil, &[], PROVIDED, NOW).unwrap();
        let planted = format!("{DIR}/targets/evil-0.0.1.kpkg");
        let bytes = other.read(&planted).unwrap().unwrap();
        vol.write(&planted, &bytes).unwrap();
        let next = artifact("next", "0.0.1", "int y;\n");
        publish(&mut vol, &key, &next, &[], PROVIDED, NOW).unwrap();
        let checked = check(&mut vol, &key, PROVIDED).unwrap();
        assert_eq!(kept(&checked), ["hello 0.0.1", "next 0.0.1"]);
        assert_eq!(vol.read(&planted), Ok(None), "the planted file is gone");
    }

    #[test]
    fn a_build_that_asks_for_anything_is_refused() {
        let mut vol = MemVolume::default();
        let routes = asking("hello", "0.0.1", "int x;\n", "", "use kai:net\n");
        let err = publish(&mut vol, &key(1), &routes, &[], PROVIDED, NOW).unwrap_err();
        assert!(err.ends_with("that asks for nothing"), "{err}");
        let twice = asking("hello", "0.0.1", "int x;\n", "", "id hello\n");
        assert!(publish(&mut vol, &key(1), &twice, &[], PROVIDED, NOW).is_err());
        let effects = asking("hello", "0.0.1", "int x;\n", "effects fs_read\n", "");
        let err = publish(&mut vol, &key(1), &effects, &[], PROVIDED, NOW).unwrap_err();
        assert_eq!(
            err,
            "hello declares effects: an app built in the OS declares none"
        );
        assert!(vol.files().is_empty());
    }

    #[test]
    fn local_holds_at_most_max_builds() {
        let mut vol = MemVolume::default();
        for n in 1..=MAX_BUILDS {
            let app = artifact(&format!("app{n}"), "0.0.1", "int x;\n");
            publish(&mut vol, &key(1), &app, &[], PROVIDED, NOW).unwrap();
        }
        let one_more = artifact("app9", "0.0.1", "int x;\n");
        let err = publish(&mut vol, &key(1), &one_more, &[], PROVIDED, NOW).unwrap_err();
        assert!(err.starts_with("local holds at most 8 builds"), "{err}");
        // A rebuild replaces its build, so it still fits.
        let rebuilt = artifact("app1", "0.0.2", "int y;\n");
        assert!(publish(&mut vol, &key(1), &rebuilt, &[], PROVIDED, NOW).is_ok());
        assert_eq!(
            next(&mut vol, Some(&key(1)), PROVIDED, "app1"),
            Ok(Version::new(0, 0, 3))
        );
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
    fn asking_the_next_version_makes_no_key() {
        let mut vol = MemVolume::default();
        assert_eq!(saved_key(&mut vol).map(|k| k.is_some()), Ok(false));
        assert_eq!(
            next(&mut vol, None, PROVIDED, "hello"),
            Ok(Version::new(0, 0, 1))
        );
        assert!(vol.files().is_empty());
        let made = device_key(&mut vol, || Ok([7; 32])).unwrap();
        let saved = saved_key(&mut vol).unwrap().unwrap();
        assert_eq!(saved.id(), made.id());
    }

    #[test]
    fn a_build_is_published_signed_by_the_device_key_and_a_rebuild_replaces_it() {
        let mut vol = MemVolume::default();
        let key = key(1);
        assert_eq!(
            next(&mut vol, Some(&key), PROVIDED, "hello"),
            Ok(Version::new(0, 0, 1))
        );
        let hello = artifact("hello", "0.0.1", "int main(void) { return 0; }\n");
        let published = publish(&mut vol, &key, &hello, &["notes"], PROVIDED, NOW).unwrap();
        assert_eq!(published.version, Version::new(0, 0, 1));
        let checked = check(&mut vol, &key, PROVIDED).unwrap();
        assert_eq!(kept(&checked), ["hello 0.0.1"]);
        assert!(checked.refused.is_empty(), "{:?}", checked.refused);
        assert_eq!(checked.indexes[0].summary, SUMMARY);
        assert_eq!(
            next(&mut vol, Some(&key), PROVIDED, "hello"),
            Ok(Version::new(0, 0, 2))
        );
        // A rebuild must be newer than the build it replaces.
        let same = artifact("hello", "0.0.1", "int main(void) { return 2; }\n");
        assert_eq!(
            publish(&mut vol, &key, &same, &[], PROVIDED, NOW)
                .err()
                .as_deref(),
            Some("hello 0.0.1 is not newer than local's hello 0.0.1")
        );
        let rebuilt = artifact("hello", "0.0.2", "int main(void) { return 1; }\n");
        assert_eq!(
            publish(&mut vol, &key, &rebuilt, &[], PROVIDED, NOW)
                .unwrap()
                .version,
            Version::new(0, 0, 2)
        );
        // A user may name a version, under the same rule.
        let named = artifact("other", "1.2.0", "int x;\n");
        publish(&mut vol, &key, &named, &[], PROVIDED, NOW).unwrap();
        assert_eq!(
            next(&mut vol, Some(&key), PROVIDED, "other"),
            Ok(Version::new(1, 2, 1)),
            "one patch past the named version, which publish takes"
        );
        let checked = check(&mut vol, &key, PROVIDED).unwrap();
        assert_eq!(kept(&checked), ["hello 0.0.2", "other 1.2.0"]);
        let targets = vol.list(&format!("{DIR}/targets")).unwrap();
        assert_eq!(targets, ["hello-0.0.2.kpkg", "other-1.2.0.kpkg"]);
    }

    #[test]
    fn a_plain_component_or_a_label_that_differs_from_the_world_is_refused() {
        let mut vol = MemVolume::default();
        let plain = component("hello", "0.0.3", "int x;\n");
        let err = publish(&mut vol, &key(1), &plain, &[], PROVIDED, NOW).unwrap_err();
        assert!(err.ends_with("only apps install from local"), "{err}");
        let app = artifact("hello", "0.0.3", "int x;\n");
        // The label is the world's version with dashes.
        let label = b"0-0-3";
        let mut bent = app.clone();
        let at = bent.windows(label.len()).position(|w| w == label).unwrap();
        bent[at..at + label.len()].copy_from_slice(b"0-0-4");
        let err = publish(&mut vol, &key(1), &bent, &[], PROVIDED, NOW).unwrap_err();
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
        let err = publish(&mut vol, &key(1), &other, &[], PROVIDED, NOW).unwrap_err();
        assert!(err.ends_with("is not its own app's"), "{err}");
        assert!(vol.files().is_empty());
    }

    #[test]
    fn a_build_cannot_take_another_repositorys_name_or_a_bad_one() {
        let mut vol = MemVolume::default();
        let notes = artifact("notes", "0.0.1", "int main(void) { return 0; }\n");
        let err = publish(&mut vol, &key(1), &notes, &["notes"], PROVIDED, NOW).unwrap_err();
        assert!(
            err.starts_with("notes is another repository's package"),
            "{err}"
        );
        let fs = artifact("fs", "0.0.1", "int x;\n");
        assert!(
            publish(&mut vol, &key(1), &fs, &[], PROVIDED, NOW).is_err(),
            "an OS row group"
        );
        assert!(publish(&mut vol, &key(1), b"not wasm", &[], PROVIDED, NOW).is_err());
        assert!(vol.files().is_empty(), "nothing is written for a refusal");
    }

    #[test]
    fn local_is_pinned_to_the_device_key_and_a_flipped_byte_is_refused_by_name() {
        let mut vol = MemVolume::default();
        let hello = artifact("hello", "0.0.1", "int main(void) { return 0; }\n");
        publish(&mut vol, &key(1), &hello, &[], PROVIDED, NOW).unwrap();
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
            let published = publish(vol, &key, &app, &[], PROVIDED, NOW).unwrap();
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
        let shas = shas(&mut vol, &key, PROVIDED).unwrap();
        assert_eq!(shas.len(), 1, "local holds its one build of hello");
    }
}
