//! The `virtio_drivers::Hal` implementation: DMA memory and buffer sharing.
//!
//! The driver hands `share` buffers from the kernel heap, from thread stacks, and
//! from boxed descriptor lists. None of those is in the direct map or physically
//! contiguous, so every shared buffer is copied through ("bounced") freshly
//! allocated contiguous frames that the device can address directly.

use crate::memory;
use core::ptr::NonNull;
use virtio_drivers::{BufferDirection, Hal, PAGE_SIZE, PhysAddr};
use x86_64::structures::paging::PageTableFlags;

/// Uncached, writable, non-executable mapping for device registers.
pub const DEVICE: PageTableFlags = PageTableFlags::PRESENT
    .union(PageTableFlags::WRITABLE)
    .union(PageTableFlags::NO_CACHE)
    .union(PageTableFlags::WRITE_THROUGH)
    .union(PageTableFlags::NO_EXECUTE);

pub struct KernelHal;

fn allocate(pages: usize) -> Option<u64> {
    memory::with(|m| m.allocate_contiguous(pages as u64))
}

fn release(phys: u64, pages: usize) -> Result<(), &'static str> {
    memory::with(|m| m.free_contiguous(phys, pages as u64))
}

fn direct_map(phys: u64) -> *mut u8 {
    (memory::hhdm() + phys) as *mut u8
}

// SAFETY: dma_alloc returns fresh, zeroed, page-aligned, physically contiguous frames
// reachable through the direct map; share bounces every buffer into frames of its own,
// so the device never sees memory that is not physically contiguous.
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
