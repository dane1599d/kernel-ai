//! Each `lifecycle 2` instance's linear memory in a region of its own, so that
//! dropping the instance (P3 task 11's suspend) gives the memory back whole.
//! The region is reserved once at the instance's cap, committed as the guest's
//! `memory.grow` asks, and released when the instance goes: the shape of lane
//! K's coming MEM_RESERVE, MEM_COMMIT and MEM_RELEASE. Until those exist the
//! three steps are on the heap ([`reserve`], [`commit`], [`release`]); they are
//! the one place that changes for real page return.
//!
//! wasmi gives a guest's own memory a heap `Vec`, so the guest's memory
//! definition is turned into an import of `kai:host/memory` ([`own_memory_as_import`]),
//! and the host supplies that import from the region.

use alloc::alloc::{Layout, alloc_zeroed, dealloc};
use alloc::vec::Vec;
use core::ptr::NonNull;

/// The import a guest's memory becomes.
pub(crate) const MODULE: &str = "kai:host";
pub(crate) const NAME: &str = "memory";
/// A WebAssembly page.
pub(crate) const PAGE: usize = 64 * 1024;
const ALIGN: usize = 4096;

/// Address space for `bytes`, nothing usable yet (MEM_RESERVE). On the heap it
/// is zeroed memory at once.
fn reserve(bytes: usize) -> Option<NonNull<u8>> {
    let layout = Layout::from_size_align(bytes, ALIGN).ok()?;
    // SAFETY: `bytes` is above 0 (a region is at least one page).
    NonNull::new(unsafe { alloc_zeroed(layout) })
}

/// Makes `[at, at + bytes)` usable, zeroed (MEM_COMMIT). The heap's region is
/// usable from the start.
fn commit(_at: NonNull<u8>, _bytes: usize) -> bool {
    true
}

/// Gives the whole region back (MEM_RELEASE).
fn release(at: NonNull<u8>, bytes: usize) {
    if let Ok(layout) = Layout::from_size_align(bytes, ALIGN) {
        // SAFETY: `at` came from `reserve(bytes)` with this layout, once.
        unsafe { dealloc(at.as_ptr(), layout) }
    }
}

/// One instance's memory region.
pub(crate) struct Region {
    at: NonNull<u8>,
    capacity: usize,
    committed: usize,
}

// SAFETY: the region is owned by one store, used by one thread at a time.
unsafe impl Send for Region {}

impl Region {
    /// A region for up to `capacity` bytes, `initial` of them usable.
    pub(crate) fn new(capacity: usize, initial: usize) -> Option<Self> {
        if capacity == 0 || initial > capacity {
            return None;
        }
        let at = reserve(capacity)?;
        let mut region = Self {
            at,
            capacity,
            committed: 0,
        };
        region.grow(initial).then_some(region)
    }

    /// The buffer wasmi's static memory runs on.
    ///
    /// # Safety
    ///
    /// The region must outlive every use of the slice: it is kept in the store's
    /// data, dropped with the store, after which the memory is not touched.
    pub(crate) unsafe fn buffer(&mut self) -> &'static mut [u8] {
        // SAFETY: `at` points at `capacity` reserved bytes; the caller keeps the
        // region alive as long as the slice is used.
        unsafe { core::slice::from_raw_parts_mut(self.at.as_ptr(), self.capacity) }
    }

    /// Commits up to `bytes`; false past the capacity or when the system has
    /// no more.
    pub(crate) fn grow(&mut self, bytes: usize) -> bool {
        if bytes > self.capacity {
            return false;
        }
        if bytes <= self.committed {
            return true;
        }
        // SAFETY: `committed < bytes <= capacity`, inside the region.
        let from = unsafe { NonNull::new_unchecked(self.at.as_ptr().add(self.committed)) };
        if !commit(from, bytes - self.committed) {
            return false;
        }
        self.committed = bytes;
        true
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        release(self.at, self.capacity);
    }
}

/// A memory's limits as the binary gives them: pages, and the maximum if set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub min: u32,
    pub max: Option<u32>,
}

fn leb(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut value: u64 = 0;
    for shift in (0..35).step_by(7) {
        let b = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return u32::try_from(value).ok();
        }
    }
    None
}

fn put_leb(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let b = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_name(out: &mut Vec<u8>, name: &str) {
    put_leb(out, name.len() as u32);
    out.extend_from_slice(name.as_bytes());
}

/// Section ids, and an import's kind byte for a memory.
const IMPORT: u8 = 2;
const MEMORY: u8 = 5;
const KIND_MEMORY: u8 = 2;

/// `wasm` with its one memory definition turned into the import
/// `kai:host/memory` of the same limits, and those limits; none when it defines
/// no memory, or one this cannot move (several, shared or 64-bit).
pub(crate) fn own_memory_as_import(wasm: &[u8]) -> Option<(Vec<u8>, Limits)> {
    let mut sections: Vec<(u8, &[u8])> = Vec::new();
    let mut at = 8;
    if wasm.get(..8)? != b"\0asm\x01\0\0\0" {
        return None;
    }
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let len = leb(wasm, &mut at)? as usize;
        let body = wasm.get(at..at.checked_add(len)?)?;
        at += len;
        sections.push((id, body));
    }
    let memory = sections.iter().position(|(id, _)| *id == MEMORY)?;
    let body = sections[memory].1;
    let mut p = 0;
    if leb(body, &mut p)? != 1 {
        return None;
    }
    let raw_start = p;
    let limits = match body.get(p)? {
        0 => {
            p += 1;
            Limits {
                min: leb(body, &mut p)?,
                max: None,
            }
        }
        1 => {
            p += 1;
            let min = leb(body, &mut p)?;
            Limits {
                min,
                max: Some(leb(body, &mut p)?),
            }
        }
        _ => return None,
    };
    let raw_limits = &body[raw_start..p];
    let mut entry = Vec::new();
    put_name(&mut entry, MODULE);
    put_name(&mut entry, NAME);
    entry.push(KIND_MEMORY);
    entry.extend_from_slice(raw_limits);
    // The new import section: the old one's entries, then the memory.
    let mut imports = Vec::new();
    match sections.iter().find(|(id, _)| *id == IMPORT) {
        Some((_, old)) => {
            let mut q = 0;
            let count = leb(old, &mut q)?;
            put_leb(&mut imports, count + 1);
            imports.extend_from_slice(&old[q..]);
        }
        None => put_leb(&mut imports, 1),
    }
    imports.extend_from_slice(&entry);
    let mut out = Vec::from(&wasm[..8]);
    let mut placed = false;
    for (id, body) in &sections {
        // The import section goes where it was, or before the first section
        // that must follow it (custom sections, id 0, may sit anywhere).
        if !placed && (*id == IMPORT || (*id > IMPORT && *id != 0)) {
            out.push(IMPORT);
            put_leb(&mut out, imports.len() as u32);
            out.extend_from_slice(&imports);
            placed = true;
        }
        if *id == IMPORT || *id == MEMORY {
            continue;
        }
        out.push(*id);
        put_leb(&mut out, body.len() as u32);
        out.extend_from_slice(body);
    }
    if !placed {
        out.push(IMPORT);
        put_leb(&mut out, imports.len() as u32);
        out.extend_from_slice(&imports);
    }
    Some((out, limits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_defined_memory_becomes_an_import_of_the_same_limits() {
        let wat = r#"(module
            (import "kai:app/log" "write" (func (param i32 i32)))
            (memory (export "memory") 2 40)
            (data (i32.const 8) "hi")
            (func (export "f") (result i32) (i32.load8_u (i32.const 8))))"#;
        let wasm = wat::parse_str(wat).unwrap();
        let (moved, limits) = own_memory_as_import(&wasm).unwrap();
        assert_eq!(
            limits,
            Limits {
                min: 2,
                max: Some(40)
            }
        );
        let engine = wasmi::Engine::default();
        let module = wasmi::Module::new(&engine, &moved).unwrap();
        let imports: Vec<_> = module.imports().map(|i| (i.module(), i.name())).collect();
        assert_eq!(imports, [("kai:app/log", "write"), (MODULE, NAME)]);
        assert!(module.exports().any(|e| e.name() == "memory"));
    }

    #[test]
    fn a_module_with_no_imports_gets_an_import_section() {
        let wasm = wat::parse_str("(module (memory 1))").unwrap();
        let (moved, limits) = own_memory_as_import(&wasm).unwrap();
        assert_eq!(limits, Limits { min: 1, max: None });
        assert!(wasmi::Module::new(&wasmi::Engine::default(), &moved).is_ok());
        assert!(own_memory_as_import(&wat::parse_str("(module)").unwrap()).is_none());
    }

    #[test]
    fn a_region_commits_up_to_its_capacity() {
        let mut r = Region::new(4 * PAGE, PAGE).unwrap();
        assert!(r.grow(2 * PAGE) && r.grow(4 * PAGE));
        assert!(!r.grow(5 * PAGE));
        assert!(Region::new(PAGE, 2 * PAGE).is_none());
    }
}
