//! What the boot banner says this kernel is: its version, the roadmap milestone
//! it was built at, and the commit xtask built it from (`KAI_COMMIT`). The banner
//! is printed before the heap exists, so all of it is constant.

/// The last milestone merged (docs/roadmap.md). The version's minor number
/// follows the milestone's: 0.10 is M10.
pub const MILESTONE: &str = "M10a";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The commit, when xtask built the kernel.
pub const COMMIT: &str = match option_env!("KAI_COMMIT") {
    Some(commit) => commit,
    None => "unknown",
};

/// The banner's words, written without allocating.
pub struct Banner;

impl core::fmt::Display for Banner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Kernel AI {VERSION} ({MILESTONE}, commit {COMMIT})")
    }
}
