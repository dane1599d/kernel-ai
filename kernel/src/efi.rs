//! UEFI runtime services (ADR 0008): GetVariable, SetVariable and ResetSystem,
//! for `bootslot`'s `KaiBootState` and Limine's `LoaderEntryOneShot`.
//!
//! - Limine hands over the system table and the firmware's memory map (base
//!   revision 6: virtual addresses in the HHDM).
//! - [`init`] maps every runtime region at `WINDOW + physical address` in the
//!   kernel half, which every address space shares: code executable, data not,
//!   MMIO (the variable store's flash) uncached. It then calls
//!   SetVirtualAddressMap once, on a temporary PML4 that also maps the regions
//!   1:1, as the call must run with the firmware's physical addresses valid.
//! - Every call after that runs under one lock with interrupts off and the
//!   x87/SSE state saved around it: the kernel is soft-float, firmware may not be.
//!
//! With no system table (or before [`init`]) the calls answer [`Error::Unavailable`]
//! and a reset falls back to `power::reboot` (ADR 0007's FADT reset register).

use crate::log;
use crate::memory;
use alloc::vec::Vec;
use limine::request::{EfiMemmapRequest, EfiRequest};
use spin::{Mutex, Once};
use x86_64::instructions::interrupts;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::PageTableFlags;

#[used]
#[unsafe(link_section = ".requests")]
static EFI: EfiRequest = EfiRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static EFI_MEMMAP: EfiMemmapRequest = EfiMemmapRequest::new();

/// Where runtime regions are mapped: physical address `p` at `WINDOW + p`.
const WINDOW: u64 = 0xFFFF_E000_0000_0000;
const PAGE: u64 = 4096;
/// EFI_MEMORY_RUNTIME.
const RUNTIME: u64 = 1 << 63;
const RUNTIME_CODE: u32 = 5;
const MMIO: u32 = 11;
const MMIO_PORT_SPACE: u32 = 12;
/// Offsets in the system table and the runtime services table (UEFI 2.10).
const SYSTEM_TABLE_RUNTIME: u64 = 88;
const RT_SET_VIRTUAL_ADDRESS_MAP: u64 = 56;
const RT_GET_VARIABLE: u64 = 72;
const RT_SET_VARIABLE: u64 = 88;
const RT_RESET_SYSTEM: u64 = 104;
/// EFI_STATUS values, with the high bit for errors.
const NOT_FOUND: usize = 0x8000_0000_0000_000E;
const BUFFER_TOO_SMALL: usize = 0x8000_0000_0000_0005;
/// Longest variable name, in UTF-16 units with its terminator.
const MAX_NAME: usize = 64;

/// A vendor GUID, as UEFI lays it out.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// Kernel AI's vendor GUID for its own variables (`KaiBootState`).
pub const KAI: Guid = Guid {
    data1: 0x6b61_6931,
    data2: 0x0008,
    data3: 0x4b41,
    data4: [0x9a, 0x2c, 0x51, 0x6f, 0x74, 0x3e, 0x0b, 0x12],
};

/// Non-volatile, readable at boot and at run time: every variable ADR 0008 writes.
pub const NV_BS_RT: u32 = 0x1 | 0x2 | 0x4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No runtime services: no EFI, or not set up.
    Unavailable,
    NotFound,
    /// The variable is bigger: its size.
    BufferTooSmall(usize),
    /// A name too long or not ASCII.
    BadName,
    /// Any other EFI_STATUS.
    Status(usize),
}

type SetVirtualAddressMap = extern "efiapi" fn(usize, usize, u32, *mut u8) -> usize;
type GetVariable =
    extern "efiapi" fn(*const u16, *const Guid, *mut u32, *mut usize, *mut u8) -> usize;
type SetVariable = extern "efiapi" fn(*const u16, *const Guid, u32, usize, *const u8) -> usize;
type ResetSystem = extern "efiapi" fn(u32, usize, usize, *const u8) -> !;

struct Services {
    get: GetVariable,
    set: SetVariable,
    reset: ResetSystem,
}

static SERVICES: Once<Services> = Once::new();
/// One firmware call at a time.
static CALL: Mutex<()> = Mutex::new(());

/// One runtime region from the firmware's memory map.
struct Region {
    kind: u32,
    phys: u64,
    pages: u64,
}

fn read_u64(addr: u64) -> u64 {
    // SAFETY: callers pass addresses of firmware tables mapped by the HHDM.
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

/// Maps the runtime services and switches them to virtual addresses. No EFI is
/// not an error: the machine then has no runtime services.
pub fn init() -> Result<(), &'static str> {
    let (Some(system), Some(map)) = (EFI.response(), EFI_MEMMAP.response()) else {
        log!("efi: no EFI system table; no runtime services");
        return Ok(());
    };
    let hhdm = memory::hhdm();
    let runtime_table = hhdm + read_u64(system.address as u64 + SYSTEM_TABLE_RUNTIME);
    let desc_size = usize::try_from(map.desc_size).map_err(|_| "efi: bad descriptor size")?;
    if desc_size < 40 {
        return Err("efi: the memory map's descriptors are too small");
    }
    let mut copy: Vec<u8> = Vec::new();
    copy.try_reserve_exact(map.memmap().len())
        .map_err(|_| "efi: no heap for the memory map")?;
    copy.extend_from_slice(map.memmap());
    let mut regions = Vec::new();
    for desc in copy.chunks_exact_mut(desc_size) {
        let field = |at: usize| u64::from_le_bytes(desc[at..at + 8].try_into().unwrap_or([0; 8]));
        if field(32) & RUNTIME == 0 {
            continue;
        }
        let region = Region {
            kind: u32::from_le_bytes(desc[0..4].try_into().unwrap_or([0; 4])),
            phys: field(8),
            pages: field(24),
        };
        desc[16..24].copy_from_slice(&(WINDOW + region.phys).to_le_bytes());
        regions.push(region);
    }
    map_window(&regions)?;
    let status = set_virtual_address_map(
        &regions,
        runtime_table,
        &mut copy,
        desc_size,
        map.desc_version,
    )?;
    if status != 0 {
        log!("efi: SetVirtualAddressMap failed: {status:#x}");
        return Err("efi: SetVirtualAddressMap failed");
    }
    // The firmware has rewritten its own pointers to the window.
    // SAFETY: each slot now holds the window address of a function with this ABI.
    let services = unsafe {
        Services {
            get: core::mem::transmute::<u64, GetVariable>(read_u64(
                runtime_table + RT_GET_VARIABLE,
            )),
            set: core::mem::transmute::<u64, SetVariable>(read_u64(
                runtime_table + RT_SET_VARIABLE,
            )),
            reset: core::mem::transmute::<u64, ResetSystem>(read_u64(
                runtime_table + RT_RESET_SYSTEM,
            )),
        }
    };
    SERVICES.call_once(|| services);
    let pages: u64 = regions.iter().map(|r| r.pages).sum();
    log!(
        "efi: runtime services at {WINDOW:#x} ({} regions, {pages} pages); SetVirtualAddressMap done",
        regions.len()
    );
    Ok(())
}

/// Maps every runtime region at its place in the window, in the kernel half.
fn map_window(regions: &[Region]) -> Result<(), &'static str> {
    memory::with(|m| {
        for region in regions {
            let mut flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
            if region.kind != RUNTIME_CODE {
                flags |= PageTableFlags::NO_EXECUTE;
            }
            if region.kind == MMIO || region.kind == MMIO_PORT_SPACE {
                flags |= PageTableFlags::NO_CACHE | PageTableFlags::WRITE_THROUGH;
            }
            for n in 0..region.pages {
                let phys = region.phys + n * PAGE;
                m.map_physical_page(WINDOW + phys, phys, flags)
                    .map_err(|_| "efi: a runtime page is already mapped in the window")?;
            }
        }
        Ok(())
    })
}

/// Calls SetVirtualAddressMap on a temporary PML4 that shares the kernel half and
/// maps the runtime regions 1:1 too; its status.
fn set_virtual_address_map(
    regions: &[Region],
    runtime_table: u64,
    map: &mut [u8],
    desc_size: usize,
    desc_version: u64,
) -> Result<usize, &'static str> {
    let mut space = memory::with(|m| m.new_user_space())?;
    let mapped = memory::with(|m| {
        regions.iter().try_for_each(|region| {
            (0..region.pages).try_for_each(|n| {
                let phys = region.phys + n * PAGE;
                m.map_firmware_page(&mut space, phys, phys)
            })
        })
    });
    let status = mapped.map(|()| {
        // SAFETY: the slot holds the firmware's physical address of a function with
        // this ABI, mapped 1:1 in the temporary PML4.
        let call = unsafe {
            core::mem::transmute::<u64, SetVirtualAddressMap>(read_u64(
                runtime_table + RT_SET_VIRTUAL_ADDRESS_MAP,
            ))
        };
        interrupts::without_interrupts(|| {
            let (kernel, flags) = Cr3::read();
            let temporary = x86_64::structures::paging::PhysFrame::containing_address(
                x86_64::PhysAddr::new(space.cr3()),
            );
            // SAFETY: the temporary PML4 shares the kernel half, so this code and its
            // stack stay mapped; interrupts are off until the kernel's is back.
            unsafe { Cr3::write(temporary, flags) };
            let status = with_fpu_saved(|| {
                call(map.len(), desc_size, desc_version as u32, map.as_mut_ptr())
            });
            // SAFETY: as above, back to the kernel's PML4.
            unsafe { Cr3::write(kernel, flags) };
            status
        })
    });
    memory::with(|m| m.destroy_user_space(space));
    status
}

/// Runs `f` with this CPU's x87/SSE state saved and restored around it.
fn with_fpu_saved<R>(f: impl FnOnce() -> R) -> R {
    #[repr(C, align(16))]
    struct Area([u8; 512]);
    let mut area = Area([0; 512]);
    // SAFETY: fxsave64 writes 512 bytes to a 16-byte-aligned area; SSE is on.
    unsafe { core::arch::asm!("fxsave64 [{}]", in(reg) area.0.as_mut_ptr(), options(nostack)) };
    let result = f();
    // SAFETY: restores what fxsave64 saved above.
    unsafe { core::arch::asm!("fxrstor64 [{}]", in(reg) area.0.as_ptr(), options(nostack)) };
    result
}

/// One firmware call, under the lock with interrupts off and the FPU saved.
fn call<R>(f: impl FnOnce(&Services) -> R) -> Result<R, Error> {
    let services = SERVICES.get().ok_or(Error::Unavailable)?;
    Ok(interrupts::without_interrupts(|| {
        let _one = CALL.lock();
        with_fpu_saved(|| f(services))
    }))
}

/// `name` as NUL-terminated UTF-16; ASCII only.
fn name16(name: &str) -> Result<[u16; MAX_NAME], Error> {
    if name.len() >= MAX_NAME || !name.is_ascii() || name.is_empty() {
        return Err(Error::BadName);
    }
    let mut out = [0u16; MAX_NAME];
    for (slot, byte) in out.iter_mut().zip(name.bytes()) {
        *slot = u16::from(byte);
    }
    Ok(out)
}

/// Reads the variable `name` under `vendor` into `data`; its length.
pub fn get_variable(vendor: &Guid, name: &str, data: &mut [u8]) -> Result<usize, Error> {
    let name = name16(name)?;
    let mut size = data.len();
    let mut attributes = 0u32;
    let status = call(|s| {
        (s.get)(
            name.as_ptr(),
            vendor,
            &mut attributes,
            &mut size,
            data.as_mut_ptr(),
        )
    })?;
    match status {
        0 => Ok(size),
        NOT_FOUND => Err(Error::NotFound),
        BUFFER_TOO_SMALL => Err(Error::BufferTooSmall(size)),
        other => Err(Error::Status(other)),
    }
}

/// Writes the variable `name` under `vendor` with `attributes`; empty `data`
/// deletes it.
pub fn set_variable(vendor: &Guid, name: &str, attributes: u32, data: &[u8]) -> Result<(), Error> {
    let name = name16(name)?;
    let status = call(|s| (s.set)(name.as_ptr(), vendor, attributes, data.len(), data.as_ptr()))?;
    match status {
        0 => Ok(()),
        NOT_FOUND => Err(Error::NotFound),
        other => Err(Error::Status(other)),
    }
}

/// Resets the machine through ResetSystem (a cold reset), or through
/// `power::reboot` without runtime services.
pub fn reset() -> ! {
    if let Some(services) = SERVICES.get() {
        log!("efi: resetting through ResetSystem");
        interrupts::disable();
        with_fpu_saved(|| (services.reset)(0, 0, 0, core::ptr::null()))
    }
    crate::power::reboot()
}
