//! Fixed places in every user address space.

/// Programs are linked here.
pub const IMAGE_BASE: u64 = 0x0040_0000;
/// The heap grows up from here.
pub const HEAP_BASE: u64 = 0x0000_0010_0000_0000;
/// Most the heap may grow, whatever the page quota.
pub const HEAP_MAX: u64 = 0x0000_0001_0000_0000;
/// The stack grows down from here; the page below the stack stays unmapped.
pub const STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
/// Default stack size.
pub const STACK_PAGES: u64 = 16;
/// Largest stack a process may ask for.
pub const MAX_STACK_PAGES: u64 = 1024;
/// The model is mapped here, read-only: the start of a PML4 slot of its own,
/// so its page tables hold nothing else.
pub const MODEL_BASE: u64 = 0x0000_2000_0000_0000;
/// Largest model that fits the window: one PML4 slot, 512 GiB.
pub const MODEL_MAX: u64 = 0x0000_0080_0000_0000;
/// `Shm` objects are mapped here, each at the start of its own 1 GiB slot
/// ([`SHM_SLOT`]): object `n` at `SHM_BASE + n * SHM_SLOT`.
pub const SHM_BASE: u64 = 0x0000_3000_0000_0000;
/// The address space each `Shm` object may take, so its largest size.
pub const SHM_SLOT: u64 = 0x0000_0000_4000_0000;
/// Anonymous reservations (MEM_RESERVE) are made from here, never reused,
/// up to [`MEM_END`]: 16 TiB, clear of the model's and `Shm`'s windows.
pub const MEM_BASE: u64 = 0x0000_4000_0000_0000;
pub const MEM_END: u64 = 0x0000_5000_0000_0000;
/// First address that is not user space.
pub const USER_END: u64 = 0x0000_8000_0000_0000;
