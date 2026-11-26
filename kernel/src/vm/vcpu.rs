//! Spike S06-3's vCPU on AMD SVM: one VMCB, nested paging over guest RAM,
//! every port and most MSRs intercepted, CPUID filtered, and an exit loop
//! that hands port I/O to [`super::devices`]. Exits are classified by the
//! `svm` crate's [`svm::dispatch`] (#1494). Throwaway (ADR 0006).

use super::devices::{Devices, Effect, Marker};
use crate::{hpet, log};
use svm::exit::{Hand, Kernel, Next};
use svm::intercept::{Intercept, Intercepts, msr_bit};
use svm::vmcb::{Segment, V_IGN_TPR, V_INTR_MASKING, V_INTR_PRIO_SHIFT, V_IRQ, Vmcb};
use svm::{Action, Exit, dispatch};
use x86_64::registers::model_specific::Msr;

/// The guest's general registers VMRUN does not switch (it switches RAX,
/// RSP and RIP through the VMCB).
#[repr(C)]
#[derive(Default)]
pub struct Gprs {
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r: [u64; 8],
}

unsafe extern "C" {
    /// CLGI; VMSAVE the host's hidden state; VMLOAD the guest's; VMRUN;
    /// VMSAVE the guest's; VMLOAD the host's; STGI.
    fn s063_vmrun(regs: *mut Gprs, guest_vmcb: u64, host_vmcb: u64);
}

core::arch::global_asm!(
    ".global s063_vmrun",
    "s063_vmrun:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "push rdi",
    "push rdx",
    "push rsi",
    "clgi",
    "mov rax, rdx",
    "vmsave rax",
    "mov rax, rsi",
    "vmload rax",
    "mov rbx, [rdi + 0]",
    "mov rcx, [rdi + 8]",
    "mov rdx, [rdi + 16]",
    "mov rsi, [rdi + 24]",
    "mov rbp, [rdi + 40]",
    "mov r8, [rdi + 48]",
    "mov r9, [rdi + 56]",
    "mov r10, [rdi + 64]",
    "mov r11, [rdi + 72]",
    "mov r12, [rdi + 80]",
    "mov r13, [rdi + 88]",
    "mov r14, [rdi + 96]",
    "mov r15, [rdi + 104]",
    "mov rdi, [rdi + 32]",
    "vmrun rax",
    "push rdi",
    "mov rdi, [rsp + 24]",
    "mov [rdi + 0], rbx",
    "mov [rdi + 8], rcx",
    "mov [rdi + 16], rdx",
    "mov [rdi + 24], rsi",
    "mov [rdi + 40], rbp",
    "mov [rdi + 48], r8",
    "mov [rdi + 56], r9",
    "mov [rdi + 64], r10",
    "mov [rdi + 72], r11",
    "mov [rdi + 80], r12",
    "mov [rdi + 88], r13",
    "mov [rdi + 96], r14",
    "mov [rdi + 104], r15",
    "pop rax",
    "mov [rdi + 32], rax",
    "pop rax",
    "vmsave rax",
    "pop rax",
    "vmload rax",
    "stgi",
    "pop rdi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
);

const EFER: u32 = 0xC000_0080;
const EFER_SVME: u64 = svm::EFER_SVME;
/// EFER bits a guest may set: SCE, LME, LMA, NXE.
const EFER_GUEST: u64 = 1 | 1 << 8 | 1 << 10 | 1 << 11;
/// MSRs VMLOAD and VMSAVE switch, so the guest has them itself.
const SWITCHED: [u32; 10] = [
    0xC000_0081, // STAR
    0xC000_0082, // LSTAR
    0xC000_0083, // CSTAR
    0xC000_0084, // SFMASK
    0xC000_0100, // FS_BASE
    0xC000_0101, // GS_BASE
    0xC000_0102, // KERNEL_GS_BASE
    0x174,       // SYSENTER_CS
    0x175,       // SYSENTER_ESP
    0x176,       // SYSENTER_EIP
];

/// The pages one vCPU needs, by host physical address.
pub struct Pages {
    pub vmcb: u64,
    pub host: u64,
    pub hsave: u64,
    pub iopm: u64,
    pub msrpm: u64,
}

/// Counts of each exit kind, for the run's summary.
#[derive(Default)]
pub struct Stats {
    pub exits: u64,
    pub io: u64,
    pub intr: u64,
    pub hlt: u64,
    pub cpuid: u64,
    pub msr: u64,
    pub other: u64,
}

/// What the loop does after an exit the kernel handled.
enum Flow {
    /// Move past the instruction.
    Advance,
    /// A fault was injected: the instruction runs again in the guest's handler.
    Fault,
    End(End),
}

/// How the guest's run ended.
pub enum End {
    /// The guest wrote the probe's done marker.
    Done,
    /// HLT with interrupts off and nothing to wake it.
    Halted,
}

/// The host times of the probe's markers, from the first VMRUN, in ns.
#[derive(Default)]
pub struct Times {
    pub start: u64,
    pub init: Option<u64>,
    pub run_init_line: Option<u64>,
    pub decode: Option<(u64, u64)>,
    pub bench: Option<(u64, u64, u64)>,
}

pub struct Vcpu<'a> {
    vmcb: &'a mut Vmcb,
    pages: Pages,
    regs: Gprs,
    nrips: bool,
    intercepts: Intercepts,
    layout: &'a svm::Layout,
    host_xcr0: u64,
    tsc_aux: u64,
    unknown_msrs: [u32; 16],
    pub stats: Stats,
    pub times: Times,
}

/// Turns SVM on for this CPU: EFER.SVME and the host save area.
pub fn enable(hsave: u64) -> Result<bool, &'static str> {
    let ecx = core::arch::x86_64::__cpuid(0x8000_0001).ecx;
    if ecx & (1 << 2) == 0 {
        return Err("s06-3: the CPU offers no SVM (run on an AMD runner or under TCG)");
    }
    let svm = core::arch::x86_64::__cpuid(svm::CPUID_SVM);
    if svm.edx & 1 == 0 {
        return Err("s06-3: SVM without nested paging");
    }
    // SAFETY: CPUID offers SVM, so VM_CR, EFER.SVME and VM_HSAVE_PA exist;
    // `hsave` is a page this vCPU owns.
    unsafe {
        if Msr::new(svm::MSR_VM_CR).read() & svm::VM_CR_SVMDIS != 0 {
            return Err("s06-3: SVM is disabled (VM_CR.SVMDIS)");
        }
        let mut efer = Msr::new(EFER);
        efer.write(efer.read() | EFER_SVME);
        Msr::new(svm::MSR_VM_HSAVE_PA).write(hsave);
    }
    Ok(svm.edx & (1 << 3) != 0)
}

impl<'a> Vcpu<'a> {
    /// A vCPU at the bzImage's 32-bit entry: protected mode, paging off,
    /// flat __BOOT_CS and __BOOT_DS, `rsi` the zero page.
    pub fn new(
        vmcb: &'a mut Vmcb,
        pages: Pages,
        layout: &'a svm::Layout,
        ncr3: u64,
        nrips: bool,
        entry: &super::linux::Entry,
    ) -> Self {
        let intercepts = Intercepts::required()
            .with(Intercept::CPUID)
            .with(Intercept::HLT)
            .with(Intercept::XSETBV)
            .with(Intercept::INVLPGA)
            .with(Intercept::VMMCALL)
            .with(Intercept::RDPMC)
            .with(Intercept::MONITOR)
            .with(Intercept::MWAIT)
            .with(Intercept::exception(18));
        let c = &mut vmcb.control;
        intercepts.apply(c);
        c.iopm_base_pa = pages.iopm;
        c.msrpm_base_pa = pages.msrpm;
        c.asid = 1;
        c.tlb_control = svm::vmcb::TLB_FLUSH_ALL;
        c.int_control = V_INTR_MASKING;
        c.nested_control = svm::vmcb::NP_ENABLE;
        c.n_cr3 = ncr3;
        let s = &mut vmcb.save;
        let code = Segment {
            selector: 0x10,
            attrib: Segment::attrib(0x9B, 0xC),
            limit: 0xFFFF_FFFF,
            base: 0,
        };
        let data = Segment {
            selector: 0x18,
            attrib: Segment::attrib(0x93, 0xC),
            ..code
        };
        s.cs = code;
        (s.ds, s.es, s.ss, s.fs, s.gs) = (data, data, data, data, data);
        s.gdtr = Segment {
            selector: 0,
            attrib: 0,
            limit: 0x1F,
            base: super::linux::GDT,
        };
        s.idtr = Segment {
            limit: 0xFFFF,
            ..Segment::default()
        };
        s.ldtr = Segment {
            attrib: 0x82,
            limit: 0xFFFF,
            ..Segment::default()
        };
        s.tr = Segment {
            attrib: 0x8B,
            limit: 0xFFFF,
            ..Segment::default()
        };
        s.efer = EFER_SVME;
        s.cr0 = 0x11;
        s.rflags = 2;
        s.rip = entry.rip;
        s.dr6 = 0xFFFF_0FF0;
        s.dr7 = 0x400;
        s.g_pat = svm::npt::PAT_POWER_ON;
        // SAFETY: the IOPM and MSRPM pages are this vCPU's, contiguous, and
        // reachable through the direct map.
        unsafe {
            let hhdm = crate::memory::hhdm();
            core::ptr::write_bytes((hhdm + pages.iopm) as *mut u8, 0xFF, svm::intercept::IOPM_SIZE);
            let msrpm = (hhdm + pages.msrpm) as *mut u8;
            core::ptr::write_bytes(msrpm, 0xFF, svm::intercept::MSRPM_SIZE);
            for msr in &SWITCHED {
                if let Some((byte, bit)) = msr_bit(*msr) {
                    *msrpm.add(byte) &= !(3 << bit);
                }
            }
        }
        let regs = Gprs {
            rsi: entry.rsi,
            ..Gprs::default()
        };
        // SAFETY: XGETBV with XSAVE on, as cpu::init made it.
        let host_xcr0 = unsafe { core::arch::x86_64::_xgetbv(0) };
        Self {
            vmcb,
            pages,
            regs,
            nrips,
            intercepts,
            layout,
            host_xcr0,
            tsc_aux: 0,
            unknown_msrs: [0; 16],
            stats: Stats::default(),
            times: Times::default(),
        }
    }

    /// Runs the guest until it is done, halts for good, or is killed.
    pub fn run(&mut self, dev: &mut Devices) -> Result<End, &'static str> {
        self.times.start = hpet::nanos();
        loop {
            let now = hpet::nanos();
            dev.update(now);
            self.offer_interrupt(dev);
            // SAFETY: the VMCB, host save page and register block are this
            // vCPU's; SVM is on (enable) on this, the only, CPU.
            unsafe { s063_vmrun(&mut self.regs, self.pages.vmcb, self.pages.host) };
            self.stats.exits += 1;
            let c = &mut self.vmcb.control;
            c.tlb_control = svm::vmcb::TLB_FLUSH_NONE;
            c.event_inj = 0;
            if let Ok(Some(event)) = svm::Event::interrupted(c) {
                event.inject(c);
            }
            let exit = Exit::read(c, self.nrips);
            let guest = svm::exit::Guest {
                intercepts: self.intercepts,
                ram: self.layout,
            };
            match dispatch(&exit, &guest) {
                Action::Kernel(what, next) => match self.kernel(what, dev)? {
                    Flow::Advance => self.advance(next)?,
                    Flow::Fault => {}
                    Flow::End(end) => return Ok(end),
                },
                Action::Vmm(Hand::Pio(io), Next::To(rip)) if !io.string => {
                    self.stats.io += 1;
                    let rax = self.vmcb.save.rax;
                    let (read, effect) = dev.io(io.port, io.size, io.input, rax as u32, hpet::nanos());
                    if io.input {
                        self.vmcb.save.rax = match io.size {
                            1 => rax & !0xFF | read as u64,
                            2 => rax & !0xFFFF | read as u64,
                            _ => read as u64,
                        };
                    }
                    self.vmcb.save.rip = rip;
                    self.vmcb.control.int_state &= !1;
                    if let Some(end) = self.effect(effect, dev) {
                        return Ok(end);
                    }
                }
                Action::Inject(fault) => {
                    self.stats.other += 1;
                    fault.event.inject(&mut self.vmcb.control);
                }
                action => {
                    log!(
                        "s06-3: guest killed: {action:?} at rip {:#x} (exit {})",
                        self.vmcb.save.rip,
                        exit.code.name()
                    );
                    return Err("s06-3: the guest was killed");
                }
            }
        }
    }

    fn advance(&mut self, next: Next) -> Result<(), &'static str> {
        match next {
            Next::Stay => return Ok(()),
            Next::To(rip) => self.vmcb.save.rip = rip,
            // Without NRIPS (TCG): the bare opcode, which is what Linux emits
            // for each of these.
            Next::Decode(insn) => self.vmcb.save.rip += insn.opcode().len() as u64,
            Next::Emulator => return Err("s06-3: an exit needs the instruction emulator"),
        }
        self.vmcb.control.int_state &= !1;
        Ok(())
    }

    /// Offers the PIC's next interrupt as the virtual interrupt, which the
    /// CPU delivers once the guest's IF allows (APM 15.21.4).
    fn offer_interrupt(&mut self, dev: &mut Devices) {
        let c = &mut self.vmcb.control;
        if c.event_inj & (1 << 31) != 0 || c.int_control & V_IRQ != 0 {
            return;
        }
        if let Some(vector) = dev.take_interrupt() {
            c.int_vector = vector as u32;
            c.int_control = V_INTR_MASKING | V_IRQ | V_IGN_TPR | 0xF << V_INTR_PRIO_SHIFT;
        }
    }

    fn kernel(&mut self, what: Kernel, dev: &mut Devices) -> Result<Flow, &'static str> {
        match what {
            Kernel::HostEvent => self.stats.intr += 1,
            Kernel::Cpuid => {
                self.stats.cpuid += 1;
                let r = cpuid(self.vmcb.save.rax as u32, self.regs.rcx as u32, self.vmcb.save.cr4);
                self.vmcb.save.rax = r[0] as u64;
                (self.regs.rbx, self.regs.rcx, self.regs.rdx) = (r[1] as u64, r[2] as u64, r[3] as u64);
            }
            Kernel::Msr { write } => {
                self.stats.msr += 1;
                let msr = self.regs.rcx as u32;
                let ok = match write {
                    true => {
                        let v = (self.regs.rdx << 32) | (self.vmcb.save.rax & 0xFFFF_FFFF);
                        self.wrmsr(msr, v)
                    }
                    false => match self.rdmsr(msr) {
                        Some(v) => {
                            self.vmcb.save.rax = v & 0xFFFF_FFFF;
                            self.regs.rdx = v >> 32;
                            true
                        }
                        None => false,
                    },
                };
                if !ok {
                    return self.gp();
                }
            }
            Kernel::Xsetbv => {
                self.stats.other += 1;
                let v = (self.regs.rdx << 32) | (self.vmcb.save.rax & 0xFFFF_FFFF);
                if self.regs.rcx != 0 || v & 1 == 0 || v & !self.host_xcr0 != 0 {
                    return self.gp();
                }
                // XCR0 stays the host's: the guest's set is within it, and
                // the FPU state is saved whole at every switch.
            }
            Kernel::Hlt => {
                self.stats.hlt += 1;
                if let Some(end) = self.halt(dev) {
                    return Ok(Flow::End(end));
                }
            }
            Kernel::Pause | Kernel::Flush | Kernel::InterruptWindow => self.stats.other += 1,
            Kernel::MachineCheck => return Err("s06-3: a machine check in the guest"),
            Kernel::Rdtsc { .. } | Kernel::Apic { .. } => {
                log!("s06-3: unexpected exit {what:?}");
                return Err("s06-3: an exit the spike does not handle");
            }
        }
        Ok(Flow::Advance)
    }

    /// HLT: waits on the host's timer until the PIC has something for the
    /// guest. With the guest's IF clear and nothing pending, it never wakes.
    fn halt(&mut self, dev: &mut Devices) -> Option<End> {
        if self.vmcb.control.int_state & 1 != 0 {
            self.vmcb.control.int_state &= !1;
        }
        // Linux halts with IF clear only to stop: after a panic or at halt.
        if self.vmcb.save.rflags & (1 << 9) == 0 {
            return Some(End::Halted);
        }
        loop {
            dev.update(hpet::nanos());
            if self.vmcb.control.int_control & V_IRQ != 0 || dev.has_interrupt() {
                return None;
            }
            x86_64::instructions::interrupts::enable_and_hlt();
        }
    }

    fn gp(&mut self) -> Result<Flow, &'static str> {
        let event = svm::Event::exception(13, Some(0)).map_err(|_| "s06-3: #GP refused")?;
        event.inject(&mut self.vmcb.control);
        Ok(Flow::Fault)
    }

    fn effect(&mut self, effect: Effect, dev: &Devices) -> Option<End> {
        let since = || hpet::nanos() - self.times.start;
        match effect {
            Effect::None => {}
            Effect::Line => {
                if dev.ran_init && self.times.run_init_line.is_none() {
                    self.times.run_init_line = Some(since());
                }
            }
            Effect::Marker(m) => {
                let t = since();
                match m {
                    Marker::Init => self.times.init = Some(t),
                    Marker::DecodeStart => self.times.decode = Some((t, t)),
                    Marker::DecodeEnd => {
                        if let Some((s, _)) = self.times.decode {
                            self.times.decode = Some((s, t));
                        }
                    }
                    Marker::BenchStart => self.times.bench = Some((t, t, dev.bench_outs)),
                    Marker::BenchEnd => {
                        if let Some((s, _, n)) = self.times.bench {
                            self.times.bench = Some((s, t, dev.bench_outs - n));
                        }
                    }
                    Marker::Done => return Some(End::Done),
                    Marker::Other(v) => log!("s06-3: probe marker {v:#x}"),
                }
            }
        }
        None
    }

    fn rdmsr(&mut self, msr: u32) -> Option<u64> {
        let s = &self.vmcb.save;
        Some(match msr {
            EFER => s.efer & !EFER_SVME,
            0x277 => s.g_pat,
            0x10 => unsafe { core::arch::x86_64::_rdtsc() },
            0x1A0 => 1, // MISC_ENABLE: fast strings.
            0xC000_0103 => self.tsc_aux,
            0xC001_0114 => svm::VM_CR_SVMDIS,
            _ => {
                self.note_msr(msr);
                0
            }
        })
    }

    fn wrmsr(&mut self, msr: u32, v: u64) -> bool {
        match msr {
            EFER => {
                if v & !EFER_GUEST != 0 {
                    return false;
                }
                self.vmcb.save.efer = v | EFER_SVME;
            }
            0x277 => self.vmcb.save.g_pat = v,
            0xC000_0103 => self.tsc_aux = v,
            _ => self.note_msr(msr),
        }
        true
    }

    fn note_msr(&mut self, msr: u32) {
        if self.unknown_msrs.contains(&msr) {
            return;
        }
        if let Some(slot) = self.unknown_msrs.iter_mut().find(|m| **m == 0) {
            *slot = msr;
            log!("s06-3: MSR {msr:#x} not emulated: reads 0, writes ignored");
        }
    }
}

/// CPUID as the guest sees it: the host's leaves, less what the spike does
/// not emulate (the APIC, MTRRs, machine checks, PMU, SVM, topology and
/// speculation controls), one CPU, and OSXSAVE from the guest's CR4.
fn cpuid(leaf: u32, sub: u32, cr4: u64) -> [u32; 4] {
    let host = |l, s| {
        let r = core::arch::x86_64::__cpuid_count(l, s);
        [r.eax, r.ebx, r.ecx, r.edx]
    };
    match leaf {
        0 => {
            let mut r = host(0, 0);
            r[0] = r[0].min(0xD);
            r
        }
        1 => {
            let mut r = host(1, 0);
            r[1] = (r[1] & 0xFFFF) | 1 << 16;
            const ECX: u32 = 1 | 1 << 1 | 1 << 9 | 1 << 12 | 1 << 13 | 1 << 19 | 1 << 20 | 1 << 22 | 1 << 23
                | 1 << 25 | 1 << 26 | 1 << 28 | 1 << 29 | 1 << 30;
            r[2] = (r[2] & ECX) | ((cr4 >> 18) as u32 & 1) << 27;
            const EDX: u32 = 1 | 1 << 2 | 1 << 3 | 1 << 4 | 1 << 5 | 1 << 6 | 1 << 8 | 1 << 11 | 1 << 13 | 1 << 15
                | 1 << 16 | 1 << 17 | 1 << 19 | 1 << 23 | 1 << 24 | 1 << 25 | 1 << 26;
            r[3] &= EDX;
            r
        }
        7 if sub == 0 => {
            let mut r = host(7, 0);
            r[0] = 0;
            const EBX: u32 = 1 | 1 << 3 | 1 << 5 | 1 << 7 | 1 << 8 | 1 << 9 | 1 << 16 | 1 << 17 | 1 << 19 | 1 << 20
                | 1 << 21 | 1 << 23 | 1 << 24 | 1 << 28 | 1 << 29 | 1 << 30 | 1 << 31;
            const ECX: u32 = 1 << 1 | 1 << 2 | 1 << 6 | 1 << 8 | 1 << 9 | 1 << 10 | 1 << 11 | 1 << 12 | 1 << 14 | 1 << 22;
            (r[1], r[2], r[3]) = (r[1] & EBX, r[2] & ECX, 0);
            r
        }
        0xD => {
            let mut r = host(0xD, sub);
            if sub == 1 {
                // No XSAVES: IA32_XSS is not emulated.
                (r[0], r[2], r[3]) = (r[0] & !(1 << 3), 0, 0);
            }
            r
        }
        0x8000_0000 => {
            let mut r = host(leaf, 0);
            r[0] = r[0].min(0x8000_0008);
            r
        }
        0x8000_0001 => {
            let mut r = host(leaf, 0);
            r[2] &= 1 | 1 << 5 | 1 << 6 | 1 << 7 | 1 << 8;
            r[3] &= 1 << 11 | 1 << 20 | 1 << 22 | 1 << 26 | 1 << 27 | 1 << 29;
            r
        }
        0x8000_0002..=0x8000_0006 => host(leaf, 0),
        0x8000_0007 => [0, 0, 0, host(leaf, 0)[3] & 1 << 8],
        0x8000_0008 => [host(leaf, 0)[0], 0, 0, 0],
        _ => [0; 4],
    }
}
