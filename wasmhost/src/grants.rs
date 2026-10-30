//! What a version is granted when it loads. A granted interface (`kai:settings`
//! today; `kai:edit` and `kai:system` add theirs) is linked only for a version
//! whose [`Grants`] allow it: without the grant, the import does not link and the
//! version does not stage (`Reason::Link`).
//!
//! The grant comes from the system image's plan line (or a plan argument comphost
//! reads from it), never from a package's own manifest, until package grants
//! exist. It holds only for the image's own versions of the component
//! ([`FromImage`]): an upload or a package version swapped in under the same
//! name runs without it.

use alloc::vec::Vec;

/// The granted interfaces a version may import.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    /// May import `kai:settings/settings`.
    pub settings: bool,
    /// Its effects a person's key or click caused go to the shell as the
    /// user's (`effects put user`): the Terminal's `shell_line`. Not an import,
    /// so nothing links on it; comphost reads it from the shown instance.
    pub shell_line: bool,
}

/// The artifacts the host fetched under a boot label: the signed image's own
/// versions, whose label is neither an upload's (`e<n>`) nor a package's
/// (`p-<hex>`). The kernel enforces those forms at every add, so no upload or
/// package version can pose as a boot one. A version is known by the SHA-256
/// of its artifact, which the caller computes; one it never fetched under a
/// boot label gets no grant (fail closed).
#[derive(Default)]
pub struct FromImage {
    digests: Vec<[u8; 32]>,
}

/// Most image artifacts remembered; the oldest is forgotten first.
pub const MAX_IMAGE_VERSIONS: usize = 16;

/// Whether `label` is a boot module's: not an upload's, not a package's.
pub fn is_boot_label(label: &str) -> bool {
    !kabi::live::is_upload_label(label) && !kabi::live::is_package_label(label)
}

impl FromImage {
    /// The artifact with SHA-256 `digest` was fetched under `label`.
    pub fn fetched(&mut self, label: &str, digest: [u8; 32]) {
        if !is_boot_label(label) || self.digests.contains(&digest) {
            return;
        }
        if self.digests.len() == MAX_IMAGE_VERSIONS {
            self.digests.remove(0);
        }
        self.digests.push(digest);
    }

    /// `grants` for the artifact with SHA-256 `digest` if it is one of the
    /// image's own, else none.
    pub fn grants_for(&self, digest: &[u8; 32], grants: Grants) -> Grants {
        match self.digests.contains(digest) {
            true => grants,
            false => Grants::default(),
        }
    }
}
