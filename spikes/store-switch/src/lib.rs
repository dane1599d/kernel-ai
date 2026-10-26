//! Spike S11-2's host half (ADR 0011 §5): does a package switch need more than one
//! ADR 0004 transaction?
//!
//! The store (`store`) keeps `pkg/store/<sha256>` objects, numbered generations in
//! `kai-gen 1`, `pkg/current`, `pkg/good` and `pkg/failed`, as ADR 0011 §5 lays them
//! out. A switch writes the new generation, `current`, the grants and the action
//! log line together. It runs over a [`Disk`]: kaifs's transactions on S04-1's
//! spike engine (`kaifs_disk`), or design C's three filesystem primitives,
//! write-and-flush, rename-replace and remove (`prims`). The harness (`harness`)
//! runs random install, remove, rollback, gc and probation sequences and crashes
//! them at every write, flush and commit, then checks the store's invariants.
//! Never merged.

pub mod harness;
pub mod kaifs_disk;
pub mod prims;
pub mod repo;
pub mod store;

/// What the store needs of a filesystem. kaifs makes everything between `begin`
/// and `commit` one transaction; design C's primitives make each call durable on
/// its own and ignore `begin` and `commit`.
pub trait Disk {
    fn begin(&mut self) {}
    fn commit(&mut self) {}
    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String>;
    /// The names of the files directly in `dir`.
    fn list(&mut self, dir: &str) -> Result<Vec<String>, String>;
    /// Writes a whole file: design C's write-and-flush. Not atomic there: a crash
    /// can leave a prefix.
    fn write(&mut self, path: &str, bytes: &[u8]);
    /// Design C's rename-replace: atomic.
    fn rename(&mut self, from: &str, to: &str);
    fn remove(&mut self, path: &str);
    /// Whether the file at `path` exists and its SHA-256 is `sha`.
    fn verified(&mut self, path: &str, sha: &str) -> Result<bool, String> {
        Ok(self
            .read(path)?
            .is_some_and(|bytes| repo::sha(&bytes) == sha))
    }
}
