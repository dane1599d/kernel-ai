//! `virtio_drivers::Hal` for `netd`: every page the card sees is in one DMA
//! arena, a kernel `Shm` object below 4 GiB whose physical address the kernel
//! put on the command line. With no IOMMU yet, a device address is that
//! physical address. Nothing is bounced: the queues and every frame buffer are
//! carved from the arena, so `share` only translates.

use core::ptr::NonNull;
use spin::Mutex;
use virtio_drivers::{BufferDirection, Hal, PAGE_SIZE, PhysAddr};

struct Arena {
    va: u64,
    phys: u64,
    len: u64,
    next: u64,
}

static ARENA: Mutex<Arena> = Mutex::new(Arena {
    va: 0,
    phys: 0,
    len: 0,
    next: 0,
});

/// The arena, mapped at `va`, at `phys` for the card.
pub fn init(va: u64, phys: u64, len: u64) {
    *ARENA.lock() = Arena {
        va,
        phys,
        len,
        next: 0,
    };
}

/// `pages` zeroed pages of the arena, never given back: its virtual and
/// physical addresses.
pub fn take(pages: usize) -> Option<(u64, u64)> {
    let mut arena = ARENA.lock();
    let bytes = (pages * PAGE_SIZE) as u64;
    if arena.next + bytes > arena.len {
        return None;
    }
    let at = arena.next;
    arena.next += bytes;
    Some((arena.va + at, arena.phys + at))
}

/// A `len`-byte buffer of the arena, for the card's frames.
pub fn buffer(len: usize) -> &'static mut [u8] {
    let (va, _) = take(len.div_ceil(PAGE_SIZE)).expect("netd: the DMA arena is full");
    // SAFETY: those pages are this buffer's alone, mapped, and never freed.
    unsafe { core::slice::from_raw_parts_mut(va as *mut u8, len) }
}

fn phys_of(va: u64, len: u64) -> PhysAddr {
    let arena = ARENA.lock();
    assert!(
        va >= arena.va && va + len <= arena.va + arena.len,
        "netd: a buffer outside the DMA arena"
    );
    arena.phys + (va - arena.va)
}

pub struct ArenaHal;

// SAFETY: dma_alloc gives zeroed, page-aligned, contiguous pages of the arena,
// which the card reaches at their physical address; share gives the physical
// address of a buffer that lies wholly in the arena, and refuses any other.
unsafe impl Hal for ArenaHal {
    fn dma_alloc(pages: usize, _: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        match take(pages) {
            Some((va, phys)) => (phys, NonNull::new(va as *mut u8).unwrap()),
            None => (0, NonNull::dangling()),
        }
    }

    unsafe fn dma_dealloc(_: PhysAddr, _: NonNull<u8>, _: usize) -> i32 {
        0
    }

    unsafe fn mmio_phys_to_virt(_: PhysAddr, _: usize) -> NonNull<u8> {
        panic!("netd: its transport maps no MMIO through the Hal")
    }

    unsafe fn share(buffer: NonNull<[u8]>, _: BufferDirection) -> PhysAddr {
        phys_of(buffer.cast::<u8>().as_ptr() as u64, buffer.len() as u64)
    }

    unsafe fn unshare(_: PhysAddr, _: NonNull<[u8]>, _: BufferDirection) {}
}
