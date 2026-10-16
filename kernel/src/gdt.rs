//! Global descriptor table and task state segment.
//!
//! The TSS gives the double-fault handler its own stack (IST entry 0), so a fault
//! caused by a bad stack pointer still has somewhere to run instead of triple-faulting.

use spin::LazyLock;
use x86_64::VirtAddr;
use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const IST_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct IstStack([u8; IST_STACK_SIZE]);

static mut DOUBLE_FAULT_STACK: IstStack = IstStack([0; IST_STACK_SIZE]);

static TSS: LazyLock<TaskStateSegment> = LazyLock::new(|| {
    let mut tss = TaskStateSegment::new();
    let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
        stack_start + IST_STACK_SIZE as u64;
    tss
});

struct Gdt {
    table: GlobalDescriptorTable,
    code: SegmentSelector,
    data: SegmentSelector,
    tss: SegmentSelector,
}

static GDT: LazyLock<Gdt> = LazyLock::new(|| {
    let mut table = GlobalDescriptorTable::new();
    let code = table.append(Descriptor::kernel_code_segment());
    let data = table.append(Descriptor::kernel_data_segment());
    let tss = table.append(Descriptor::tss_segment(&TSS));
    Gdt {
        table,
        code,
        data,
        tss,
    }
});

/// Loads the GDT, reloads the segment registers and loads the TSS.
pub fn init() {
    GDT.table.load();
    // SAFETY: the selectors index descriptors in the GDT that was just loaded.
    unsafe {
        CS::set_reg(GDT.code);
        SS::set_reg(GDT.data);
        DS::set_reg(GDT.data);
        ES::set_reg(GDT.data);
        load_tss(GDT.tss);
    }
}
