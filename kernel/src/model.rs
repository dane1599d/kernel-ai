//! The model: the file on the `kai-model` disk, held in memory for the `llm`
//! service.
//!
//! At boot, before the process page budget is set, the kernel copies the disk into
//! frames that belong to no process and are never freed, so they count against no
//! quota and survive every restart of `llm`. It checks only the GGUF magic and
//! version; ring 3 parses everything else, as untrusted input. MODEL_MAP maps the
//! frames read-only and non-executable at `kabi::layout::MODEL_BASE` for the holder
//! of `cap::MODEL`, without the OWNED bit, so tearing a process down never frees
//! them.

use crate::virtio::blk::{self, Disk};
use crate::{hpet, log, memory, process};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::Error;
use kabi::model::{Attached, HeaderError, SERIAL};
use mm::PAGE_SIZE;
use mm::dma::{SECTOR, sectors_for};
use spin::Once;

/// The one service, and program, that may map the model.
pub const HOLDER: &str = "llm";
/// Pages the llm service gets before the page tables of the model's mapping.
pub const HOLDER_PAGES: u64 = 1024;
/// Frames that must stay free after the copy: 192 MiB. Processes may hold half of
/// what is free, and the default plan's services can ask for about 70 MiB.
const RESERVE_FRAMES: u64 = 48 * 1024;

pub struct Model {
    len: u64,
    /// Frame numbers, in file order; page `i` of the file is in `frames[i]`.
    frames: Vec<u32>,
}

static MODEL: Once<Model> = Once::new();

/// The loaded model, if the boot found one.
pub fn get() -> Option<&'static Model> {
    MODEL.get()
}

impl Model {
    /// Length of the file in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Physical addresses of the frames, in file order.
    pub fn frames(&self) -> impl Iterator<Item = u64> + '_ {
        self.frames
            .iter()
            .map(|&frame| u64::from(frame) * PAGE_SIZE)
    }
}

/// Copies the `kai-model` disk into memory, if there is one and it holds a GGUF
/// file. Anything else is logged and ignored: the system runs without a model.
pub fn load(cmdline: &str) {
    let attached = Attached::from_cmdline(cmdline);
    let started = hpet::nanos();
    match blk::with_disk(SERIAL, |disk| copy(disk, attached.len)) {
        None => {}
        Some(Ok(model)) => {
            log!(
                "model: {} bytes of GGUF v3 copied from the kai-model disk into {} frames in {} ms",
                model.len,
                model.frames.len(),
                (hpet::nanos() - started) / 1_000_000
            );
            MODEL.call_once(|| model);
        }
        Some(Err(why)) => log!("model: {why}; the kai-model disk is ignored"),
    }
}

fn copy(disk: &mut Disk, attached: Option<u64>) -> Result<Model, String> {
    let sectors = disk.sectors();
    // QEMU rounds the file up to whole sectors; model_len= says where it ends.
    let len = match attached {
        Some(len) if sectors_for(len) == sectors => len,
        Some(len) => {
            return Err(format!(
                "model_len={len} does not fit a disk of {sectors} sectors"
            ));
        }
        None => sectors.checked_mul(SECTOR).ok_or("the disk is too large")?,
    };
    if len == 0 || len > kabi::layout::MODEL_MAX {
        return Err(format!("{len} bytes is no size for a model"));
    }
    // llm must be able to hold the mapping's page tables in its quota.
    if HOLDER_PAGES + kabi::model::table_pages(len) > process::MAX_PAGES_PER_PROCESS {
        return Err(format!(
            "{len} bytes need more page tables than one process may hold"
        ));
    }
    let first = disk.read(0, 1)?;
    let head = &first[..first.len().min(usize::try_from(len).unwrap_or(usize::MAX))];
    match kabi::model::check_header(head) {
        Ok(()) => {}
        Err(HeaderError::Short) => return Err(String::from("too short for a GGUF header")),
        Err(HeaderError::NoMagic) => return Err(String::from("no GGUF magic")),
        Err(HeaderError::Version(version)) => {
            return Err(format!(
                "GGUF version {version}, not {}",
                kabi::model::VERSION
            ));
        }
    }
    let pages = len.div_ceil(PAGE_SIZE);
    let free = memory::with(|m| m.free_frames());
    if pages.saturating_add(RESERVE_FRAMES) > free {
        return Err(format!(
            "{} MiB do not fit in {} MiB of free memory",
            pages / 256,
            free / 256
        ));
    }
    let mut frames = Vec::new();
    let count = usize::try_from(pages).map_err(|_| "too many pages")?;
    frames
        .try_reserve_exact(count)
        .map_err(|_| "no kernel heap for the frame list")?;
    let hhdm = memory::hhdm();
    let copied = disk.stream(len, |_, bytes| {
        // Each read starts on a page boundary (reads are 512 KiB), so every
        // piece but the file's last is a whole page.
        memory::with(|m| {
            for piece in bytes.chunks(PAGE_SIZE as usize) {
                let phys = m.allocate_frame().ok_or("out of frames")?;
                let Ok(number) = u32::try_from(phys / PAGE_SIZE) else {
                    let _ = m.free_frame(phys);
                    return Err("a frame above 16 TiB");
                };
                frames.push(number);
                let page = (hhdm + phys) as *mut u8;
                // SAFETY: the frame was just allocated for the model alone and the
                // direct map covers it; the copy and the zeroing stay inside it, so
                // the tail of the last page holds zeros, never stale memory.
                unsafe {
                    core::ptr::copy_nonoverlapping(piece.as_ptr(), page, piece.len());
                    core::ptr::write_bytes(
                        page.add(piece.len()),
                        0,
                        PAGE_SIZE as usize - piece.len(),
                    );
                }
            }
            Ok(())
        })
    });
    if let Err(why) = copied {
        memory::with(|m| {
            for &frame in &frames {
                let _ = m.free_frame(u64::from(frame) * PAGE_SIZE);
            }
        });
        return Err(String::from(why));
    }
    Ok(Model { len, frames })
}

/// The MODEL_MAP call: maps the model into the calling process and returns its
/// length. Only the `llm` service running the `llm` program may, on top of the
/// capability check the dispatcher made.
pub fn map(slot: usize) -> Result<u64, Error> {
    let me = process::identity(slot);
    if me.name.as_str() != HOLDER || me.program.as_str() != HOLDER {
        return Err(Error::Permission);
    }
    let model = MODEL.get().ok_or(Error::NotFound)?;
    process::map_shared(
        slot,
        kabi::layout::MODEL_BASE,
        &model.frames,
        kabi::model::table_pages(model.len),
    )?;
    Ok(model.len)
}
