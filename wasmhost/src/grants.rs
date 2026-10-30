//! What a version is granted when it loads. A granted interface (`kai:settings`
//! and `kai:edit`; `kai:system` adds its own) is linked only for a version
//! whose [`Grants`] allow it: without the grant, the import does not link and the
//! version does not stage (`Reason::Link`).
//!
//! The grant comes from the system image's plan line (or a plan argument comphost
//! reads from it), never from a package's own manifest, until package grants
//! exist.

/// The granted interfaces a version may import.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    /// May import `kai:settings/settings`.
    pub settings: bool,
    /// May import `kai:system/system` (the System monitor only).
    pub system: bool,
    /// May import `kai:edit/edit`.
    pub edit: bool,
}
