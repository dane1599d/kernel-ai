//! Global descriptor tables and task state segments, one set per CPU.
//!
//! Layout, fixed because SYSCALL and the entry stubs depend on it: null, kernel code
//! (0x08), kernel data (0x10), user data (0x1b), user code (0x23), TSS (0x28).
//!
//! Each TSS gives the double-fault, NMI and machine-check handlers their own stacks
//! (IST entries 0-2), so a fault caused by a bad stack pointer still has somewhere
//! to run. RSP0, the stack the CPU switches to on entry from ring 3, is set to the
//! running thread's kernel stack on every context switch. The TSS has no I/O bitmap,
//! so ring 3 can never use `in` or `out`. The boot CPU's tables are static because
//! they are loaded before the heap exists; application processors get heap-allocated
//! tables.

use crate::memory::stack::GuardedStack;
use crate::percpu::{self, MAX_CPUS};
use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicPtr, Ordering};
use spin::LazyLock;
use x86_64::VirtAddr;
use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const NMI_IST_INDEX: u16 = 1;
pub const MACHINE_CHECK_IST_INDEX: u16 = 2;
const IST_COUNT: usize = 3;
const IST_STACK_SIZE: usize = 16 * 1024;
const AP_IST_PAGES: u64 = 4;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_DATA: u16 = 0x18 | 3;
pub const USER_CODE: u16 = 0x20 | 3;

struct CpuTables {
    gdt: GlobalDescriptorTable,
    /// Written by its own CPU (RSP0 on every switch), so it needs interior mutability.
    tss: UnsafeCell<TaskStateSegment>,
    tss_selector: SegmentSelector,
}

// SAFETY: the GDT is read-only once built; the TSS is written only by the CPU that
// owns it, with interrupts off, through the pointer in its PerCpu.
unsafe impl Sync for CpuTables {}

impl CpuTables {
    /// Fills in the GDT once the TSS sits at its final, `'static` address.
    fn build(tables: &'static mut CpuTables, ist_tops: [u64; IST_COUNT]) -> &'static CpuTables {
        for (slot, top) in ist_tops.into_iter().enumerate() {
            tables.tss.get_mut().interrupt_stack_table[slot] = VirtAddr::new(top);
        }
        let tss: *const TaskStateSegment = tables.tss.get();
        let selectors = [
            tables.gdt.append(Descriptor::kernel_code_segment()),
            tables.gdt.append(Descriptor::kernel_data_segment()),
            tables.gdt.append(Descriptor::user_data_segment()),
            tables.gdt.append(Descriptor::user_code_segment()),
        ];
        let expected = [KERNEL_CODE, KERNEL_DATA, USER_DATA, USER_CODE];
        assert!(
            selectors.iter().zip(expected).all(|(s, e)| s.0 == e),
            "GDT layout does not match the selectors SYSCALL relies on"
        );
        // SAFETY: the TSS lives inside `tables`, which is 'static and never moves. It
        // is written later only through the raw pointer kept in PerCpu.
        tables.tss_selector = tables
            .gdt
            .append(unsafe { Descriptor::tss_segment_unchecked(tss) });
        tables
    }

    const fn empty() -> Self {
        Self {
            gdt: GlobalDescriptorTable::new(),
            tss: UnsafeCell::new(TaskStateSegment::new()),
            tss_selector: SegmentSelector::NULL,
        }
    }

    fn load(&'static self) {
        self.gdt.load();
        let tss = self.tss.get();
        percpu::this().tss.store(tss as u64, Ordering::Relaxed);
        // SAFETY: the selectors index descriptors in the GDT that was just loaded.
        // FS and GS are left alone: loading a selector would reset GS_BASE.
        unsafe {
            CS::set_reg(SegmentSelector(KERNEL_CODE));
            SS::set_reg(SegmentSelector(KERNEL_DATA));
            DS::set_reg(SegmentSelector(KERNEL_DATA));
            ES::set_reg(SegmentSelector(KERNEL_DATA));
            load_tss(self.tss_selector);
        }
    }
}

#[repr(C, align(16))]
struct IstStacks([[u8; IST_STACK_SIZE]; IST_COUNT]);

static mut BOOT_IST_STACKS: IstStacks = IstStacks([[0; IST_STACK_SIZE]; IST_COUNT]);
static mut BOOT_TABLES: CpuTables = CpuTables::empty();

static BOOT: LazyLock<&'static CpuTables> = LazyLock::new(|| {
    let base = (&raw const BOOT_IST_STACKS) as u64;
    let tops = core::array::from_fn(|i| base + ((i + 1) * IST_STACK_SIZE) as u64);
    // SAFETY: BOOT_TABLES is only ever reached through this LazyLock, once.
    let tables: *mut CpuTables = &raw mut BOOT_TABLES;
    CpuTables::build(unsafe { &mut *tables }, tops)
});

static AP_TABLES: [AtomicPtr<CpuTables>; MAX_CPUS] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_CPUS];

/// Loads the boot CPU's GDT and TSS. Per-CPU data must already exist.
pub fn init() {
    BOOT.load();
}

/// Builds an application processor's tables, with guarded IST stacks. Runs on the
/// boot CPU before that processor starts.
pub fn prepare_ap(index: usize) -> Result<(), &'static str> {
    let mut tops = [0; IST_COUNT];
    for top in &mut tops {
        *top = GuardedStack::new(AP_IST_PAGES)?.top;
    }
    let tables = CpuTables::build(Box::leak(Box::new(CpuTables::empty())), tops);
    AP_TABLES[index].store(
        tables as *const CpuTables as *mut CpuTables,
        Ordering::Release,
    );
    Ok(())
}

/// Loads an application processor's GDT and TSS, on that processor.
pub fn load_ap(index: usize) {
    let tables = AP_TABLES[index].load(Ordering::Acquire);
    // SAFETY: prepare_ap stored a leaked, 'static CpuTables before starting this CPU.
    let tables: &'static CpuTables = unsafe { &*tables };
    tables.load();
}

/// Sets the stack this CPU switches to when an interrupt or exception arrives from
/// ring 3. Called with interrupts off on every context switch.
pub fn set_ring0_stack(top: u64) {
    let tss = percpu::this().tss.load(Ordering::Relaxed) as *mut TaskStateSegment;
    if tss.is_null() {
        return;
    }
    // SAFETY: the pointer is this CPU's TSS, set by `load`; only this CPU writes it,
    // with interrupts off. The TSS is packed, so the field may be unaligned.
    unsafe {
        (&raw mut (*tss).privilege_stack_table)
            .cast::<u64>()
            .write_unaligned(top)
    };
}
