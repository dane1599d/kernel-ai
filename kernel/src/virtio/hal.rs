//! The `virtio_drivers::Hal` implementation: DMA memory and buffer sharing.
//!
//! The drivers hand `share` buffers from the kernel heap, from thread stacks, and
//! from boxed descriptor lists. None of those is in the direct map or physically
//! contiguous, so they are copied through ("bounced") a slot of the bounce pool
//! ([`mm::bounce`], P6 G66) that the device can address directly. A buffer that
//! lies wholly in the direct map, such as the block driver's staging buffer, is
//! already physically contiguous, and the device reads or writes it in place.
//!
//! `share` cannot fail: the queue notifies the device right after it. So it never
//! allocates. Each virtio wrapper calls [`add_slots`] at init for as many buffers
//! as its queues can hold shared at once ([`mm::bounce::queue_slots`]), and fails
//! its init if it cannot get them; and each bounces only buffers of at most
//! [`SLOT_BYTES`], which its own constants assert at compile time.

use crate::{log, memory, threads};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use mm::bounce::{BouncePool, SLOT_BYTES};
use spin::Mutex;
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

/// The bounce slots every wrapper added, shared by all of them: `share` does not
/// know which device it shares for.
static POOL: Mutex<BouncePool> = Mutex::new(BouncePool::new());

/// Slots per run of contiguous frames that [`add_slots`] asks for: 16 pages, so
/// a fragmented memory map still has room for them.
const RUN_SLOTS: u64 = 32;

/// Sets aside `slots` more bounce slots for good, for a wrapper's queues; an
/// error (with what was already added kept) when the frames or the pool's lists
/// cannot be had.
pub fn add_slots(slots: u64) -> Result<(), &'static str> {
    let per_page = mm::PAGE_SIZE / SLOT_BYTES;
    let mut left = slots;
    while left > 0 {
        let run = left.min(RUN_SLOTS);
        let pages = run.div_ceil(per_page);
        let phys = allocate(pages as usize).ok_or("virtio: out of frames for bounce slots")?;
        let added = with_pool(|pool| pool.add(phys, pages * per_page));
        if added.is_err() {
            // The frames were not handed out; give them back.
            let _ = release(phys, pages as usize);
            return Err("virtio: the bounce pool cannot track more slots");
        }
        left -= run;
    }
    Ok(())
}

/// The bounce slots, and how many are free now.
pub fn bounce_slots() -> (usize, usize) {
    with_pool(|pool| (pool.slots(), pool.free()))
}

fn with_pool<R>(f: impl FnOnce(&mut BouncePool) -> R) -> R {
    // Never preempted holding the lock, so another thread on this CPU cannot
    // spin on it.
    threads::preempt_disable();
    let result = f(&mut POOL.lock());
    threads::preempt_enable();
    result
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
        // Neither can happen while every wrapper keeps to its slots and refuses
        // larger buffers first (see the module's docs); the queue would notify the
        // device of whatever came back, so there is nothing safe to return.
        if len as u64 > SLOT_BYTES {
            panic!("virtio: a {len}-byte buffer to bounce, over the {SLOT_BYTES}-byte slots");
        }
        let Some(phys) = with_pool(BouncePool::take) else {
            panic!("virtio: every bounce slot is taken; a queue holds more than it added");
        };
        if direction != BufferDirection::DeviceToDriver {
            // SAFETY: the driver guarantees the buffer is valid for `len` bytes; the
            // slot is ours until `unshare` and at least that large.
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
            // SAFETY: `phys` came from `share` for this same buffer length, a slot
            // of at least that many bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(direct_map(phys), buffer.cast::<u8>().as_ptr(), len)
            };
        }
        if let Err(error) = with_pool(|pool| pool.give(phys)) {
            log!("virtio: an unshare of {phys:#x} that is no taken bounce slot ({error:?})");
        }
    }
}
