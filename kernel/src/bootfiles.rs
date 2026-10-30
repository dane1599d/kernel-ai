//! The boot drive's files: `boot/kai.bundle` in `kai-bundle 1` (ADR 0008), with
//! every entry's SHA-256 checked, or, on a boot drive from before it, one
//! Limine module per file. Either way the rest of the kernel sees the same
//! `(path, bytes)` pairs, the bundle's paths under `/boot/` as Limine names them.
//!
//! [`open`] runs before the heap exists (the line table is read first), so it
//! keeps the checked bundle without allocating; [`init`] lists the files once
//! the heap is up.

use crate::log;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Once;

fn modules() -> &'static [&'static limine::file::File] {
    crate::MODULES.response().map_or(&[][..], |r| r.modules())
}

static BUNDLE: Once<Option<kaibundle::Bundle<'static>>> = Once::new();
static FILES: Once<Vec<(String, &'static [u8])>> = Once::new();

/// Finds and checks the bundle among Limine's modules, without allocating.
/// Fails on a bundle that does not parse or whose contents do not match their
/// digests.
pub fn open() -> Result<(), &'static str> {
    let file = modules()
        .iter()
        .find(|file| file.path().ends_with("/kai.bundle"));
    let bundle = match file {
        Some(file) => {
            let data: &'static [u8] = file.data();
            let bundle = kaibundle::parse(data).map_err(|error| {
                log!("boot: kai.bundle: {error:?}");
                "boot: kai.bundle is damaged"
            })?;
            log!(
                "boot: kai.bundle: {} files, every SHA-256 checked",
                bundle.len()
            );
            Some(bundle)
        }
        None => None,
    };
    BUNDLE.call_once(|| bundle);
    Ok(())
}

/// The file whose path ends with `name`, before the heap exists.
pub fn find(name: &str) -> Option<&'static [u8]> {
    match BUNDLE.get().copied().flatten() {
        Some(bundle) => bundle
            .entries()
            .find(|e| e.path.ends_with(name))
            .map(|e| e.data),
        None => modules()
            .iter()
            .find(|file| file.path().ends_with(name))
            .map(|file| file.data()),
    }
}

/// Lists the files, once the heap is up.
pub fn init() {
    let files = match BUNDLE.get().copied().flatten() {
        Some(bundle) => bundle
            .entries()
            .map(|entry| (format!("/boot/{}", entry.path), entry.data))
            .collect(),
        None => modules()
            .iter()
            .map(|file| (String::from(file.path()), file.data()))
            .collect(),
    };
    FILES.call_once(|| files);
}

/// The file at `path` exactly, a path from the bundle's top (`/boot/` as Limine
/// names it), once the heap is up.
pub fn exact(path: &str) -> Option<&'static [u8]> {
    all()
        .find(|(p, _)| p.strip_prefix("/boot/") == Some(path))
        .map(|(_, data)| data)
}

/// The files, as `(path, bytes)`.
pub fn all() -> impl Iterator<Item = (&'static str, &'static [u8])> + Clone {
    FILES
        .get()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|(path, data)| (path.as_str(), *data))
}
