//! The `virtio_drivers::Hal` implementation: DMA memory and buffer sharing.
//!
//! The drivers hand `share` buffers from the kernel heap, from thread stacks, and
//! from boxed descriptor lists. None of those is in the direct map or physically
//! contiguous, so they are copied through ("bounced") freshly allocated contiguous
//! frames that the device can address directly. A buffer that lies wholly in the
//! direct map, such as the block driver's staging buffer, is already physically
//! contiguous, and the device reads or writes it in place.

use crate::memory;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use virtio_drivers::{BufferDirection, Hal, PAGE_SIZE, PhysAddr};
use x86_64::structures::paging::PageTableFlags;

/// Uncached, writable, non-executable mapping for device registers.
pub const DEVICE: PageTableFlags = PageTableFlags::PRESENT
    .union(PageTableFlags::WRITABLE)
    .union(PageTableFlags::NO_CACHE)
    .union(PageTableFlags::WRITE_THROUGH)
    .union(PageTableFlags::NO_EXECUTE);

pub struct KernelHal;

/// Buffers handed to a device in place, without a bounce copy, since boot.
static IN_PLACE: AtomicU64 = AtomicU64::new(0);

/// How many buffers went to a device in place since boot.
pub fn in_place_shares() -> u64 {
    IN_PLACE.load(Ordering::Relaxed)
}

fn allocate(pages: usize) -> Option<u64> {
    memory::with(|m| m.allocate_contiguous(pages as u64))
}

fn release(phys: u64, pages: usize) -> Result<(), &'static str> {
    memory::with(|m| m.free_contiguous(phys, pages as u64))
}

fn direct_map(phys: u64) -> *mut u8 {
    (memory::hhdm() + phys) as *mut u8
}

/// The physical address of `buffer` when the device can use it in place.
fn in_place(buffer: NonNull<[u8]>) -> Option<u64> {
    let virt = buffer.cast::<u8>().as_ptr() as u64;
    mm::dma::direct_map_phys(
        virt,
        buffer.len() as u64,
        memory::hhdm(),
        memory::phys_end(),
    )
}

// SAFETY: dma_alloc returns fresh, zeroed, page-aligned, physically contiguous frames
// reachable through the direct map. share hands the device either the buffer itself,
// when it lies wholly in the direct map (where a virtual range is a physical range),
// or frames of its own holding a copy, so the device never sees memory that is not
// physically contiguous.
unsafe impl Hal for KernelHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let Some(phys) = allocate(pages) else {
            // Physical address 0 tells the driver the allocation failed (frame 0 is reserved).
            return (0, NonNull::dangling());
        };
        let virt = direct_map(phys);
        // SAFETY: the frames were just allocated and are mapped by the direct map.
        unsafe { core::ptr::write_bytes(virt, 0, pages * PAGE_SIZE) };
        (
            phys,
            NonNull::new(virt).expect("direct-map address is not null"),
        )
    }

    unsafe fn dma_dealloc(phys: PhysAddr, _virt: NonNull<u8>, pages: usize) -> i32 {
        match release(phys, pages) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    }

    unsafe fn mmio_phys_to_virt(phys: PhysAddr, size: usize) -> NonNull<u8> {
        let virt = memory::map_physical(phys, size as u64, DEVICE)
            .expect("virtio: cannot map a device register region");
        NonNull::new(virt as *mut u8).expect("device window address is not null")
    }

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        if let Some(phys) = in_place(buffer) {
            IN_PLACE.fetch_add(1, Ordering::Relaxed);
            return phys;
        }
        let len = buffer.len();
        let phys =
            allocate(len.div_ceil(PAGE_SIZE)).expect("virtio: out of frames for a bounce buffer");
        if direction != BufferDirection::DeviceToDriver {
            // SAFETY: the driver guarantees the buffer is valid for `len` bytes; the
            // bounce frames are ours and at least that large.
            unsafe {
                core::ptr::copy_nonoverlapping(buffer.cast::<u8>().as_ptr(), direct_map(phys), len)
            };
        }
        phys
    }

    unsafe fn unshare(phys: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
        // The same buffer gets the same answer as in `share`: used in place, there
        // is nothing to copy back or free.
        if in_place(buffer) == Some(phys) {
            return;
        }
        let len = buffer.len();
        if direction != BufferDirection::DriverToDevice {
            // SAFETY: `phys` came from `share` for this same buffer length.
            unsafe {
                core::ptr::copy_nonoverlapping(direct_map(phys), buffer.cast::<u8>().as_ptr(), len)
            };
        }
        release(phys, len.div_ceil(PAGE_SIZE)).expect("virtio: bounce buffer was not allocated");
    }
}
