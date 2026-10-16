//! Global descriptor tables and task state segments, one set per CPU.
//!
//! Each TSS gives that CPU's double-fault handler its own stack (IST entry 0), so a
//! fault caused by a bad stack pointer still has somewhere to run instead of
//! triple-faulting. The boot CPU's tables are static because they are loaded before
//! the heap exists; application processors get heap-allocated tables.

use crate::memory::stack::GuardedStack;
use crate::percpu::MAX_CPUS;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, Ordering};
use spin::LazyLock;
use x86_64::VirtAddr;
use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const IST_STACK_SIZE: usize = 16 * 1024;
const AP_IST_PAGES: u64 = 4;

struct CpuTables {
    gdt: GlobalDescriptorTable,
    tss: TaskStateSegment,
    code: SegmentSelector,
    data: SegmentSelector,
    tss_selector: SegmentSelector,
}

impl CpuTables {
    /// Fills in the GDT once the TSS sits at its final, `'static` address.
    fn build(tables: &'static mut CpuTables, double_fault_stack_top: u64) -> &'static CpuTables {
        tables.tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            VirtAddr::new(double_fault_stack_top);
        // SAFETY: the TSS lives inside `tables`, which is 'static and never moves.
        let tss_ptr: *const TaskStateSegment = &raw const tables.tss;
        let tss: &'static TaskStateSegment = unsafe { &*tss_ptr };
        tables.code = tables.gdt.append(Descriptor::kernel_code_segment());
        tables.data = tables.gdt.append(Descriptor::kernel_data_segment());
        tables.tss_selector = tables.gdt.append(Descriptor::tss_segment(tss));
        tables
    }

    const fn empty() -> Self {
        Self {
            gdt: GlobalDescriptorTable::new(),
            tss: TaskStateSegment::new(),
            code: SegmentSelector::NULL,
            data: SegmentSelector::NULL,
            tss_selector: SegmentSelector::NULL,
        }
    }

    fn load(&'static self) {
        self.gdt.load();
        // SAFETY: the selectors index descriptors in the GDT that was just loaded.
        unsafe {
            CS::set_reg(self.code);
            SS::set_reg(self.data);
            DS::set_reg(self.data);
            ES::set_reg(self.data);
            load_tss(self.tss_selector);
        }
    }
}

#[repr(C, align(16))]
struct IstStack([u8; IST_STACK_SIZE]);

static mut BOOT_DOUBLE_FAULT_STACK: IstStack = IstStack([0; IST_STACK_SIZE]);
static mut BOOT_TABLES: CpuTables = CpuTables::empty();

static BOOT: LazyLock<&'static CpuTables> = LazyLock::new(|| {
    let top = (&raw const BOOT_DOUBLE_FAULT_STACK) as u64 + IST_STACK_SIZE as u64;
    // SAFETY: BOOT_TABLES is only ever reached through this LazyLock, once.
    let tables: *mut CpuTables = &raw mut BOOT_TABLES;
    CpuTables::build(unsafe { &mut *tables }, top)
});

static AP_TABLES: [AtomicPtr<CpuTables>; MAX_CPUS] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_CPUS];

/// Loads the boot CPU's GDT and TSS.
pub fn init() {
    BOOT.load();
}

/// Builds an application processor's tables, with a guarded double-fault stack.
/// Runs on the boot CPU before that processor starts.
pub fn prepare_ap(index: usize) -> Result<(), &'static str> {
    let stack = GuardedStack::new(AP_IST_PAGES)?;
    let tables = CpuTables::build(Box::leak(Box::new(CpuTables::empty())), stack.top);
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
