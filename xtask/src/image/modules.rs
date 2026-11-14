//! One module list for `run`, `vbox` and `release-image` (founder test 2,
//! test 5): each builds its modules past the image's repository its own way,
//! then takes the list from [`image_modules`], which holds every image to
//! carrying its signed repository, so `pkg` never refuses an image for want of
//! one (`pkg: FAIL the image has no metadata/1.root.json`).

use crate::esp::Module;

/// Where `pkg` finds the image's own root: `kaipkg::image::ROOT`, beneath
/// `pkgrepo/`.
pub const SIGNED_ROOT: &str = "pkgrepo/metadata/1.root.json";
/// Where a release carries the root that update disks and the online
/// repository are checked against (`kernel/src/process/syscall/update.rs`).
pub const TRUST_ANCHOR: &str = "kai.root.json";

/// The image a module list is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Image {
    /// `cargo xtask run` and the boot cases: a fresh test root over every
    /// built-in app and model (`pkgrepo::image`).
    Run,
    /// `cargo xtask vbox`: the same, for VirtualBox.
    Vbox,
    /// `cargo xtask release-image`: the founder-signed `release/pkgrepo`, with
    /// its root as [`TRUST_ANCHOR`] too (`release_image::repository`).
    Release,
}

/// `modules`, then `repository`, the image's signed repository beneath
/// `pkgrepo/`. Run's and vbox's must hold [`SIGNED_ROOT`]. A release's holds
/// none only before the founder's repository is committed, and then carries no
/// [`TRUST_ANCHOR`] either.
pub fn image_modules(
    image: Image,
    modules: Vec<Module>,
    repository: Vec<Module>,
) -> Result<Vec<Module>, String> {
    let has = |path: &str| repository.iter().any(|m| m.path == path);
    match (image, has(SIGNED_ROOT), has(TRUST_ANCHOR)) {
        (_, true, _) | (Image::Release, false, false) => {}
        (image, false, _) => {
            return Err(format!(
                "the {image:?} image has no {SIGNED_ROOT}, so pkg would refuse it"
            ));
        }
    }
    Ok(modules.into_iter().chain(repository).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkgrepo::{self, Keys, models, tuf};
    use kaipkg::generation::Time;
    use kaipkg::sign::{self, ADR_0008};

    fn module(path: &str) -> Module {
        Module {
            path: path.into(),
            bytes: b"x".to_vec(),
        }
    }

    /// What `run` and `vbox` sign: every model of the image under a fresh test
    /// root, laid out as the image holds it (`pkgrepo::image`, less the packed
    /// apps, which need the components built).
    fn test_root_repository() -> Vec<Module> {
        let models = models::for_image().unwrap();
        let keys = Keys::generate().unwrap();
        let repository = tuf::build_with_models(&[], &models, &keys, Time(1_800_000_000)).unwrap();
        pkgrepo::as_modules(pkgrepo::image_layout(repository).unwrap()).unwrap()
    }

    /// A committed `release/pkgrepo`, as the founder's two keys sign it, and
    /// the modules `release-image` makes of it.
    fn release_repository() -> Vec<Module> {
        let root = sign::Key::from_seed([1; 32]);
        let release = sign::Key::from_seed([2; 32]);
        let keys = sign::Keys {
            root: &root,
            models: Some(&release),
            ..sign::Keys::one(&release)
        };
        let models = tuf::sign_models(&models::catalog().unwrap()).unwrap();
        let files = sign::build_with_models(&[], &models, &keys, Time(1_800_000_000), ADR_0008)
            .unwrap()
            .into_iter()
            .filter(|(path, _)| path.starts_with("metadata/"))
            .collect();
        crate::release_image::repository(Some(files), None).unwrap()
    }

    /// Every image ships a signed root where `pkg` reads it; a release also
    /// carries the same root as its trust anchor.
    #[test]
    fn run_vbox_and_release_each_ship_the_signed_root() {
        let boot = || vec![module("user/pkg.elf"), module("kai.slot")];
        for (image, repository) in [
            (Image::Run, test_root_repository()),
            (Image::Vbox, test_root_repository()),
            (Image::Release, release_repository()),
        ] {
            let modules = image_modules(image, boot(), repository).unwrap();
            let root = modules.iter().find(|m| m.path == SIGNED_ROOT);
            assert!(root.is_some(), "{image:?} ships {SIGNED_ROOT}");
            if image == Image::Release {
                let anchor = modules.iter().find(|m| m.path == TRUST_ANCHOR).unwrap();
                assert_eq!(anchor.bytes, root.unwrap().bytes);
            }
            assert_eq!(
                &modules[0].path, "user/pkg.elf",
                "{image:?} keeps the rest first"
            );
        }
    }

    /// Run and vbox never go without a signed root. A release does only
    /// before the founder's repository is committed, and never with a trust
    /// anchor but no repository.
    #[test]
    fn an_image_without_its_signed_root_is_refused() {
        for image in [Image::Run, Image::Vbox] {
            let refused = image_modules(image, vec![module("user/pkg.elf")], Vec::new())
                .err()
                .expect("refused");
            assert!(refused.contains(SIGNED_ROOT), "{refused}");
        }
        assert!(image_modules(Image::Release, Vec::new(), Vec::new()).is_ok());
        let anchor_alone = vec![module(TRUST_ANCHOR)];
        assert!(image_modules(Image::Release, Vec::new(), anchor_alone).is_err());
    }
}
