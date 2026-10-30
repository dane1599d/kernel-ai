//! The OS rows an app's effects may call (P4, the manager's M4: deny by
//! default). The app's artifact declares them on its `kai.actions` section's
//! `effects` line, which the OS enforces when the app runs. The index entry's
//! `effects` carries the same list, so the install card can show it before the
//! bytes are here. Once they are, [`check`] holds the two to each other, as
//! cargo's rule holds the header to the entry.

use crate::bundle;
use crate::header::{Header, Kind};
use alloc::string::String;
use alloc::vec::Vec;

/// What `object`'s artifact declares: none for a package that is not an `app`,
/// an app without an artifact (nothing of it runs), or an artifact without
/// `kai.actions` (a `lifecycle 1` version).
pub fn declared(header: &Header, object: &[u8]) -> Result<Vec<String>, String> {
    if header.kind != Kind::App {
        return Ok(Vec::new());
    }
    let files = bundle::files(object)?;
    let Some((_, artifact)) = files.iter().find(|(path, _)| *path == bundle::ARTIFACT) else {
        return Ok(Vec::new());
    };
    let parsed = livecore::artifact::parse(artifact)?;
    match parsed.actions {
        Some(text) => Ok(livecore::actions::parse(text)?.effects),
        None => Ok(Vec::new()),
    }
}

/// Whether `object`'s artifact declares exactly `indexed`, its index entry's
/// list: the install card showed that list, so the app gets no row it didn't.
pub fn check(header: &Header, object: &[u8], indexed: &[String]) -> Result<(), String> {
    match declared(header, object)? == indexed {
        true => Ok(()),
        false => Err("its artifact's effects are not its index entry's".into()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::resolve::Entry;
    use alloc::format;
    use alloc::string::ToString;
    use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION, SOURCE_SECTION};
    use livecore::artifact::{Manifest, append_custom_section};

    const HEADER: &str = "kai-package 1\nname files\nversion 1.0.0\nkind app\n\
        summary Your files\nlicence MPL-2.0\nworld kai:app@0.1\n";

    /// `files` 1.0.0, whose artifact's `kai.actions` has `effects` (a line, or
    /// nothing), and the package holding it.
    pub(crate) fn app(effects: &str) -> (Header, Vec<u8>) {
        let mut module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
        let manifest = Manifest {
            lifecycle: 2,
            component: "files".into(),
            version: "v1".into(),
            world: "app:files/files@1.0.0".into(),
            engine: "wasmi".into(),
            features: "-".into(),
            licence: "MPL-2.0".into(),
            build: "0123456789abcdef".into(),
            source: "fedcba9876543210".into(),
        };
        append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
        let source = livecore::artifact::bundle(&[("src/lib.rs", b"// files\n")]);
        append_custom_section(&mut module, SOURCE_SECTION, &source);
        let actions = format!("kai-actions 1\nworld app:files/files@1.0.0\n{effects}");
        append_custom_section(&mut module, ACTIONS_SECTION, actions.as_bytes());
        let object = bundle::build(&[
            (bundle::HEADER, HEADER.as_bytes()),
            (bundle::ARTIFACT, &module),
        ]);
        (Header::parse(HEADER).unwrap(), object)
    }

    #[test]
    fn an_apps_effects_are_exactly_what_its_artifact_declares() {
        let (header, object) = app("effects fs_list fs_read\n");
        let listed = ["fs_list".to_string(), "fs_read".to_string()];
        assert_eq!(declared(&header, &object).unwrap(), listed);
        assert_eq!(check(&header, &object, &listed), Ok(()));
        // An entry listing fewer rows than the artifact declares, or more, or
        // none, is refused: the card would not show what the app can call.
        assert!(check(&header, &object, &listed[..1]).is_err());
        assert!(check(&header, &object, &[]).is_err());
        let (header, none) = app("");
        assert_eq!(declared(&header, &none).unwrap(), Vec::<String>::new());
        assert!(check(&header, &none, &listed).is_err());
    }

    #[test]
    fn a_package_that_is_not_an_app_declares_none() {
        let text =
            "kai-package 1\nname words\nversion 1.0.0\nkind data\nsummary Words\nlicence MIT\n";
        let header = Header::parse(text).unwrap();
        let object = bundle::build(&[(bundle::HEADER, text.as_bytes()), ("words.txt", b"a\n")]);
        assert_eq!(declared(&header, &object), Ok(Vec::new()));
        // An app without an artifact runs nothing, so it declares nothing.
        let (app_header, _) = app("");
        assert_eq!(declared(&app_header, &object), Ok(Vec::new()));
    }

    struct NoLedger;

    impl crate::store::Ledger for NoLedger {
        fn switched(
            &mut self,
            _: &mut dyn crate::store::Volume,
            _: u64,
            _: &crate::generation::Generation,
        ) -> Result<(), String> {
            Ok(())
        }
        fn collected(
            &mut self,
            _: &mut dyn crate::store::Volume,
            _: &[u64],
            _: usize,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    struct Bytes<'a>(&'a [u8]);

    impl crate::store::Source for Bytes<'_> {
        fn fetch(&mut self, _: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
            let start = (offset as usize).min(self.0.len());
            Ok(self.0[start..(start + len).min(self.0.len())].to_vec())
        }
    }

    /// The store installs `files` from `index`'s entry listing `effects`.
    fn install(object: &[u8], effects: &[&str]) -> Result<u64, crate::store::Error> {
        let mut vol = crate::memvol::MemVolume::default();
        install_on(&mut vol, object, effects)
    }

    /// [`install`] on `vol`, which may hold the object already.
    fn install_on(
        vol: &mut crate::memvol::MemVolume,
        object: &[u8],
        effects: &[&str],
    ) -> Result<u64, crate::store::Error> {
        let mut ledger = NoLedger;
        let mut cx = crate::store::Ctx {
            vol,
            ledger: &mut ledger,
            now: crate::generation::Time(100),
            action: 1,
        };
        let store = crate::store::Store::default();
        store.setup(&mut cx, Vec::new())?;
        let entry = Entry {
            name: "files".into(),
            version: crate::version::Version::new(1, 0, 0),
            sha: crate::sha256(object),
            size: object.len() as u64,
            depends: Vec::new(),
            grants: Vec::new(),
            effects: effects.iter().map(|e| e.to_string()).collect(),
            yanked: false,
        };
        let want = [crate::resolve::Want {
            name: "files",
            range: None,
        }];
        store.install(&mut cx, &mut Bytes(object), &[entry], &[], &want, false)
    }

    #[test]
    fn the_store_refuses_an_app_whose_entry_lists_other_effects() {
        let (_, object) = app("effects fs_list fs_read\n");
        assert_eq!(install(&object, &["fs_list", "fs_read"]), Ok(2));
        let refused = || {
            let why = crate::store::Refused::EffectsDiffer("files".into());
            Err(crate::store::Error::Refused(why))
        };
        assert_eq!(install(&object, &["fs_list"]), refused());
        assert_eq!(install(&object, &[]), refused());
    }

    #[test]
    fn an_object_already_stored_is_held_to_each_new_entrys_effects() {
        let (_, object) = app("effects fs_list fs_read\n");
        let mut vol = crate::memvol::MemVolume::default();
        assert_eq!(
            install_on(&mut vol, &object, &["fs_list", "fs_read"]),
            Ok(2)
        );
        // Undone, its object stays in the store for the next install.
        let mut cx = crate::store::Ctx {
            vol: &mut vol,
            ledger: &mut NoLedger,
            now: crate::generation::Time(100),
            action: 1,
        };
        assert_eq!(crate::store::Store::default().rollback(&mut cx, 1), Ok(3));
        // An entry for the same bytes that lists fewer rows is refused, though
        // nothing is downloaded: the card would show less than it calls.
        let why = crate::store::Refused::EffectsDiffer("files".into());
        assert_eq!(
            install_on(&mut vol, &object, &["fs_read"]),
            Err(crate::store::Error::Refused(why))
        );
        assert_eq!(
            install_on(&mut vol, &object, &["fs_list", "fs_read"]),
            Ok(4)
        );
    }
}
