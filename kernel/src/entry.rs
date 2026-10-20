//! Assembly entry and exit for interrupts, exceptions and system calls.
//!
//! Every way into the kernel builds the same [`TrapFrame`] on the current thread's
//! kernel stack and leaves through the same exit path, so a thread interrupted in
//! ring 3 can be switched away from inside a handler and resumed on any CPU.
//!
//! GS rules: in the kernel GS_BASE always points at this CPU's `PerCpu`. Every entry
//! from ring 3 runs `swapgs` before anything uses GS, and the exit path runs it again
//! just before `iretq` to ring 3. Two windows need care:
//! - The `iretq` to ring 3 can itself fault (#GP, #NP or #SS) while GS already holds
//!   the user value and CS is still the kernel's. The common entry recognises a fault
//!   at [`user_iret`] by its RIP and swaps GS back as if the fault came from ring 3.
//! - NMI, double fault and machine check use IST stacks and can arrive anywhere,
//!   including between `swapgs` and `iretq`, so they decide from the GS_BASE MSR
//!   itself ("paranoid" entry). They are all fatal and never return.

use crate::gdt::{USER_CODE, USER_DATA};
use crate::percpu::{KERNEL_RSP_OFFSET, USER_RSP_OFFSET};

/// Registers saved on entry, lowest address first. `vector` is the IDT vector, or
/// [`SYSCALL_VECTOR`] for a system call.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// Marks a frame built by the SYSCALL entry; no IDT vector is this large.
pub const SYSCALL_VECTOR: u64 = 0x100;

impl TrapFrame {
    pub fn user_mode(&self) -> bool {
        self.cs & 3 == 3
    }

    /// A #GP, #NP or #SS raised by the `iretq` that was returning to ring 3.
    pub fn is_user_iret_fault(&self) -> bool {
        !self.user_mode()
            && self.rip == user_iret as *const () as u64
            && matches!(self.vector, 11..=13)
    }
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 22 * 8);

unsafe extern "C" {
    /// The `iretq` that returns to ring 3 (a label inside `common_entry`).
    fn user_iret();
}

/// Entry for every non-IST vector, after the stub pushed (error code, vector).
#[unsafe(naked)]
unsafe extern "C" fn common_entry() {
    core::arch::naked_asm!(
        // [rsp] vector, [rsp+8] error code, [rsp+16] RIP, [rsp+24] CS.
        "test byte ptr [rsp + 24], 3",
        "jnz 2f",
        "push rax",
        "lea rax, [rip + user_iret]",
        "cmp [rsp + 24], rax",
        "pop rax",
        "jne 3f",
        "2:",
        "swapgs",
        "3:",
        // Do not speculate past the swapgs decision with the wrong GS.
        "lfence",
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "cld",
        "mov rdi, rsp",
        "call {dispatch}",
        ".globl return_from_trap",
        "return_from_trap:",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        // No interrupt may arrive once GS holds the user value.
        "cli",
        "test byte ptr [rsp + 24], 3",
        "jz 4f",
        "swapgs",
        "add rsp, 16",
        ".globl user_iret",
        "user_iret:",
        "iretq",
        "4:",
        "add rsp, 16",
        "iretq",
        dispatch = sym crate::idt::trap_dispatch,
    )
}

/// SYSCALL lands here with IF, TF, DF, NT, AC and IOPL cleared (SFMASK), the user
/// RIP in RCX and RFLAGS in R11, still on the user stack. Builds a [`TrapFrame`] on
/// the thread's kernel stack and returns with `iretq`, never `sysretq`, so a
/// non-canonical return address can never fault in ring 0.
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        "swapgs",
        "mov gs:[{user_rsp}], rsp",
        "mov rsp, gs:[{kernel_rsp}]",
        "push {user_data}",
        "push qword ptr gs:[{user_rsp}]",
        "push r11",
        "push {user_code}",
        "push rcx",
        "push 0",
        "push {vector}",
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "cld",
        "mov rdi, rsp",
        "call {dispatch}",
        "jmp return_from_trap",
        user_rsp = const USER_RSP_OFFSET,
        kernel_rsp = const KERNEL_RSP_OFFSET,
        user_data = const USER_DATA,
        user_code = const USER_CODE,
        vector = const SYSCALL_VECTOR,
        dispatch = sym crate::process::syscall::dispatch,
    )
}

/// Enters ring 3 for the first time at `rip` with stack `rsp`, `arg` in RDI, every
/// other register zero and RFLAGS = IF. The frame is built at the top of the
/// thread's kernel stack, which holds nothing else from here on. The caller has
/// loaded the process's CR3.
///
/// # Safety
/// `rip` and `rsp` must be valid user addresses in the loaded address space, and the
/// thread must own nothing on its kernel stack (this never returns).
#[unsafe(naked)]
pub unsafe extern "C" fn enter_user(rip: u64, rsp: u64, arg: u64) -> ! {
    core::arch::naked_asm!(
        "cli",
        "mov rax, gs:[{kernel_rsp}]",
        "mov rsp, rax",
        "push {user_data}",
        "push rsi",
        "push 0x202",
        "push {user_code}",
        "push rdi",
        // Error code, vector, then RAX, RBX, RCX, RDX and RSI zero; RDI is `arg`.
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push rdx",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "push 0",
        "jmp return_from_trap",
        kernel_rsp = const KERNEL_RSP_OFFSET,
        user_data = const USER_DATA,
        user_code = const USER_CODE,
    )
}

/// Paranoid entry for NMI, double fault and machine check: GS may hold either value
/// whatever CS says, so read GS_BASE and swap if it is not a kernel address. FSGSBASE
/// is off, so ring 3 cannot make its GS base look like one.
#[unsafe(naked)]
unsafe extern "C" fn paranoid_entry() {
    core::arch::naked_asm!(
        "push rax",
        "push rcx",
        "push rdx",
        "mov ecx, 0xC0000101",
        "rdmsr",
        "test edx, edx",
        "js 1f",
        "swapgs",
        "1:",
        "lfence",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "cld",
        "mov rdi, rsp",
        "call {fatal}",
        "ud2",
        fatal = sym crate::idt::fatal_dispatch,
    )
}

macro_rules! stub {
    ($name:ident, $vector:expr, error_code) => {
        #[unsafe(naked)]
        pub unsafe extern "C" fn $name() {
            core::arch::naked_asm!("push {v}", "jmp {entry}", v = const $vector, entry = sym common_entry)
        }
    };
    ($name:ident, $vector:expr) => {
        #[unsafe(naked)]
        pub unsafe extern "C" fn $name() {
            core::arch::naked_asm!("push 0", "push {v}", "jmp {entry}", v = const $vector, entry = sym common_entry)
        }
    };
    ($name:ident, $vector:expr, paranoid, error_code) => {
        #[unsafe(naked)]
        pub unsafe extern "C" fn $name() {
            core::arch::naked_asm!("push {v}", "jmp {entry}", v = const $vector, entry = sym paranoid_entry)
        }
    };
    ($name:ident, $vector:expr, paranoid) => {
        #[unsafe(naked)]
        pub unsafe extern "C" fn $name() {
            core::arch::naked_asm!("push 0", "push {v}", "jmp {entry}", v = const $vector, entry = sym paranoid_entry)
        }
    };
}

stub!(vector_0, 0);
stub!(vector_1, 1);
stub!(vector_2, 2, paranoid);
stub!(vector_3, 3);
stub!(vector_4, 4);
stub!(vector_5, 5);
stub!(vector_6, 6);
stub!(vector_7, 7);
stub!(vector_8, 8, paranoid, error_code);
stub!(vector_9, 9);
stub!(vector_10, 10, error_code);
stub!(vector_11, 11, error_code);
stub!(vector_12, 12, error_code);
stub!(vector_13, 13, error_code);
stub!(vector_14, 14, error_code);
stub!(vector_15, 15);
stub!(vector_16, 16);
stub!(vector_17, 17, error_code);
stub!(vector_18, 18, paranoid);
stub!(vector_19, 19);
stub!(vector_20, 20);
stub!(vector_21, 21, error_code);
stub!(vector_22, 22);
stub!(vector_23, 23);
stub!(vector_24, 24);
stub!(vector_25, 25);
stub!(vector_26, 26);
stub!(vector_27, 27);
stub!(vector_28, 28);
stub!(vector_29, 29, error_code);
stub!(vector_30, 30, error_code);
stub!(vector_31, 31);
stub!(vector_timer, crate::apic::TIMER_VECTOR);
stub!(vector_keyboard, crate::apic::KEYBOARD_VECTOR);
stub!(vector_reschedule, crate::apic::RESCHEDULE_VECTOR);
stub!(vector_tlb, crate::apic::TLB_VECTOR);
stub!(vector_halt, crate::apic::HALT_VECTOR);
stub!(vector_spurious, crate::apic::SPURIOUS_VECTOR);
stub!(vector_mouse, crate::apic::MOUSE_VECTOR);

/// The 32 exception stubs, indexed by vector.
pub const EXCEPTIONS: [unsafe extern "C" fn(); 32] = [
    vector_0, vector_1, vector_2, vector_3, vector_4, vector_5, vector_6, vector_7, vector_8,
    vector_9, vector_10, vector_11, vector_12, vector_13, vector_14, vector_15, vector_16,
    vector_17, vector_18, vector_19, vector_20, vector_21, vector_22, vector_23, vector_24,
    vector_25, vector_26, vector_27, vector_28, vector_29, vector_30, vector_31,
];
