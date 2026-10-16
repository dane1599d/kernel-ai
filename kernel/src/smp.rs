//! Application processors: start-up through Limine, inter-processor interrupts,
//! TLB shootdown, and stopping the other CPUs on a fatal error.

use crate::percpu::{self, MAX_CPUS};
use crate::{apic, gdt, hpet, idt, log, threads, timer};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use limine::mp::MpInfo;
use spin::Mutex;

const START_TIMEOUT_NS: u64 = 2_000_000_000;
const SHOOTDOWN_TIMEOUT_NS: u64 = 1_000_000_000;

static ONLINE: AtomicUsize = AtomicUsize::new(1);
static LAPIC_IDS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

pub fn online() -> usize {
    ONLINE.load(Ordering::Acquire)
}

/// Records the boot CPU's local APIC ID (CPU index 0).
pub fn set_boot_lapic(lapic_id: u32) {
    LAPIC_IDS[0].store(lapic_id, Ordering::Relaxed);
    percpu::get(0).lapic_id.store(lapic_id, Ordering::Relaxed);
}

/// Starts every other CPU Limine reported, one at a time.
pub fn start_aps(cpus: &[&MpInfo], bsp_lapic_id: u32) -> Result<usize, &'static str> {
    let others = cpus.iter().filter(|cpu| cpu.lapic_id != bsp_lapic_id);
    for (offset, cpu) in others.enumerate() {
        let index = offset + 1;
        if index >= MAX_CPUS {
            log!("smp: more than {MAX_CPUS} CPUs; ignoring the rest");
            break;
        }
        LAPIC_IDS[index].store(cpu.lapic_id, Ordering::Relaxed);
        gdt::prepare_ap(index)?;
        let idle = threads::prepare_ap(index)?;
        cpu.bootstrap(ap_entry, ((idle as u64) << 32) | index as u64);
        let start = hpet::nanos();
        while online() <= index {
            if hpet::nanos() - start > START_TIMEOUT_NS {
                return Err("smp: an application processor did not come online");
            }
            core::hint::spin_loop();
        }
    }
    Ok(online())
}

/// Entry point for application processors. Limine runs it on its own stack with
/// interrupts disabled and our page tables loaded.
unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let argument = info.extra_argument();
    let index = (argument & 0xFFFF_FFFF) as usize;
    let idle = (argument >> 32) as usize;
    percpu::init(index);
    percpu::this()
        .lapic_id
        .store(info.lapic_id, Ordering::Relaxed);
    gdt::load_ap(index);
    idt::init();
    apic::init_ap();
    timer::init_ap();
    threads::start_ap(index, idle, || {
        ONLINE.fetch_add(1, Ordering::Release);
    })
}

pub fn send(cpu: usize, vector: u8) {
    apic::send_ipi(LAPIC_IDS[cpu].load(Ordering::Relaxed), vector);
}

/// Stops every other CPU, so a fatal report is not interleaved with other output.
pub fn halt_others() {
    let me = percpu::index();
    for cpu in (0..online()).filter(|&cpu| cpu != me) {
        send(cpu, apic::HALT_VECTOR);
    }
}

static SHOOTDOWN: Mutex<()> = Mutex::new(());
static SHOOTDOWN_ADDRESS: AtomicU64 = AtomicU64::new(0);
static SHOOTDOWN_PENDING: AtomicUsize = AtomicUsize::new(0);

/// Invalidates `address` in every other CPU's TLB and waits until they all have.
/// The caller has already flushed its own TLB entry.
pub fn tlb_shootdown(address: u64) {
    let cpus = online();
    if cpus <= 1 {
        return;
    }
    let _guard = SHOOTDOWN.lock();
    let me = percpu::index();
    SHOOTDOWN_ADDRESS.store(address, Ordering::Release);
    SHOOTDOWN_PENDING.store(cpus - 1, Ordering::Release);
    for cpu in (0..cpus).filter(|&cpu| cpu != me) {
        send(cpu, apic::TLB_VECTOR);
    }
    let start = hpet::nanos();
    while SHOOTDOWN_PENDING.load(Ordering::Acquire) > 0 {
        if hpet::nanos() - start > SHOOTDOWN_TIMEOUT_NS {
            log!("smp: TLB shootdown for {address:#x} timed out");
            return;
        }
        core::hint::spin_loop();
    }
}

/// TLB shootdown interrupt handler body.
pub fn on_tlb_ipi() {
    let address = SHOOTDOWN_ADDRESS.load(Ordering::Acquire);
    x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(address));
    SHOOTDOWN_PENDING.fetch_sub(1, Ordering::AcqRel);
}
