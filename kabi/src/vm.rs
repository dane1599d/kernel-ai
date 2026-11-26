//! `kabi::vm` version 1 (ADR 0006 item 3): the records of the one `VM` call,
//! and the entries of the `VMEXIT` KRNG v1 ring ([`crate::ring::VMEXIT`]).
//! Only the host half: the call has no number until the hypervisor batch is
//! approved, and no kernel code reads these yet.
//!
//! - **The call.** `VM` takes its operation in RDI ([`op`]) and a pointer to one
//!   record in RSI, its length in RDX. Every record starts with an 8-byte
//!   header: [`VERSION`] (u16), the operation (u16), and a u32 that is zero.
//!   The operation is in both places, so [`decode_for`] takes RDI's and refuses
//!   a header that names another. Each operation's record has one exact
//!   length; [`decode`] refuses a record
//!   that is short, over-long, of another version or operation, has a reserved
//!   byte set or a field out of range, with a [`Refused`], never a panic.
//! - **The exits.** One standard 64-byte [`crate::ring::Request`] per exit
//!   ([`ExitEntry`]); `vmm` answers with the standard 16-byte
//!   [`crate::ring::Response`], checked by [`check_reply`]. Read data goes in
//!   the ring's data area at [`data_slot`]; that area is at least
//!   `entries * 8` bytes, one 8-byte slot per entry, or the ring is refused.
//! - All integers are little-endian.
//!
//! ## Where ADR 0006 leaves a field open, the narrowest reading
//!
//! Each is a v1 rule a later version may widen, never one a v1 peer may stretch.
//!
//! - **Operations.** Only the six ADR 0006 names: `create`, `set_regs`, `run`,
//!   `irq`, `stop` and `destroy`. There is no `get_regs` and no separate RAM
//!   map: guest RAM is one `Shm` allocated in full at `create`.
//! - **Handles travel in call registers, not records.** `create` allocates the
//!   guest RAM `Shm` itself and returns it (ADR 0006 item 3), so it is not a
//!   field here; the `VMEXIT` ring is not a field either.
//! - **`create`.** `vcpus` is 1 to [`MAX_VCPUS`] (4, the ADR's default).
//!   `ram_mib` is a multiple of [`RAM_STEP_MIB`] (2 MiB, one NPT large page)
//!   from 2 to [`MAX_RAM_MIB`] (1,536, the ADR's default). No `flags` bit is
//!   defined in v1, so `flags` is 0.
//! - **`set_regs`** loads one vCPU for the Linux 64-bit boot protocol, the only
//!   guest ADR 0006 names. The state must already be long mode (CR0.PE and PG,
//!   CR4.PAE, EFER.LME and LMA). RFLAGS.VM is never set, and CR0.NW only
//!   with CD. Only the bits that boot needs may be set:
//!   [`CR0_ALLOWED`], [`CR4_ALLOWED`] and [`EFER_ALLOWED`]; EFER.SVME never is.
//!   RFLAGS has bit 1 set and no reserved bit. CR3 is 4 KiB aligned and below
//!   2^52, since CR4.PCIDE is not allowed. RIP and the GDT and IDT bases are
//!   canonical. `cs` and `ds` are GDT selectors at RPL 0 that lie inside the
//!   GDT's limit. Their hidden parts are the kernel's: flat 64-bit code, and
//!   data for DS, ES, SS, FS and GS.
//! - **`run`, `stop` and `destroy`** carry nothing past the header: they act on
//!   the VM the handle names.
//! - **`irq`** raises one of [`IOAPIC_PINS`] IOAPIC pins at level 0 or 1, or
//!   sends one MSI. Its address is in the local APICs' window
//!   (`0xFEE0_0000..=0xFEEF_FFFF`), and its data's upper 16 bits are zero.
//! - **Exits.** `args[0]` is a vCPU below [`MAX_VCPUS`]. `args[2]`, `args[3]`,
//!   `_r` and `buffer` are 0, and `flags` has no bit but [`ring::vmexit::WRITE`].
//!   - MMIO: an access of 1, 2, 4 or 8 bytes below 2^52.
//!   - Port I/O: 1, 2 or 4 bytes inside the 64 KiB port space.
//!   - Doorbell: a write of 2 or 4 bytes (virtio 1.2's notify width).
//!   - A read has no data; a write's data fits its width.
//!   - Shutdown and TripleFault carry only the vCPU.
//! - **Replies.** `status` is 0 or a negative [`crate::Error`] code. A read
//!   answered 0 has `len` equal to its size and its data in the slot, with
//!   every byte past that size zero. Any other answer has `len` 0.

use crate::ring::{self, Request, Response, vmexit};

/// The version of every record here.
pub const VERSION: u16 = 1;

/// The operations, the `VM` call's RDI.
pub mod op {
    pub const CREATE: u16 = 1;
    pub const SET_REGS: u16 = 2;
    pub const RUN: u16 = 3;
    pub const IRQ: u16 = 4;
    pub const STOP: u16 = 5;
    pub const DESTROY: u16 = 6;
}

/// Most vCPUs a VM has in v1.
pub const MAX_VCPUS: u16 = 4;
/// Guest RAM comes in steps of this many MiB.
pub const RAM_STEP_MIB: u32 = 2;
/// Most guest RAM in v1.
pub const MAX_RAM_MIB: u32 = 1536;
/// IOAPIC pins an `irq` may raise.
pub const IOAPIC_PINS: u8 = 24;
/// The local APICs' MSI address window.
pub const MSI_FIRST: u64 = 0xFEE0_0000;
pub const MSI_LAST: u64 = 0xFEEF_FFFF;

/// Bytes of the header every record starts with.
pub const HEADER_BYTES: usize = 8;
/// Bytes of each operation's record.
pub const CREATE_BYTES: usize = 24;
pub const REGS_BYTES: usize = 216;
pub const IRQ_BYTES: usize = 24;
/// `run`, `stop` and `destroy`: the header alone.
pub const BARE_BYTES: usize = HEADER_BYTES;
/// The longest record.
pub const MAX_CALL_BYTES: usize = REGS_BYTES;

/// CR0: PE, MP, EM, TS, ET, NE, WP, AM, NW, CD and PG.
pub const CR0_ALLOWED: u64 = 0x3F | (1 << 16) | (1 << 18) | (1 << 29) | (1 << 30) | (1 << 31);
const CR0_NW: u64 = 1 << 29;
const CR0_CD: u64 = 1 << 30;
const CR0_NEEDED: u64 = 1 | (1 << 31);
/// CR4: PSE, PAE, PGE, OSFXSR and OSXMMEXCPT.
pub const CR4_ALLOWED: u64 = (1 << 4) | (1 << 5) | (1 << 7) | (1 << 9) | (1 << 10);
const CR4_NEEDED: u64 = 1 << 5;
/// EFER: SCE, LME, LMA and NXE.
pub const EFER_ALLOWED: u64 = 1 | (1 << 8) | (1 << 10) | (1 << 11);
const EFER_NEEDED: u64 = (1 << 8) | (1 << 10);
/// RFLAGS bits that exist: everything up to ID (bit 21) but the reserved 3, 5
/// and 15. Bit 1 reads as 1.
const RFLAGS_DEFINED: u64 = ((1 << 22) - 1) & !((1 << 3) | (1 << 5) | (1 << 15) | RFLAGS_VM);
/// RFLAGS.VM (virtual-8086 mode): Intel VM entry refuses it in long mode.
const RFLAGS_VM: u64 = 1 << 17;
const RFLAGS_ONE: u64 = 1 << 1;
/// Guest-physical addresses are below this.
const PHYS_LIMIT: u64 = 1 << 52;

/// Why a record or an entry was refused. Every one is `Invalid` to the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Shorter or longer than its operation's record or the 64-byte entry.
    Length,
    /// Not [`VERSION`].
    Version,
    /// No such operation or exit kind.
    Op,
    /// A reserved byte, or a field this operation or kind does not use, is set.
    Reserved,
    /// A field is outside the range written in the module doc.
    Range,
}

impl From<Refused> for crate::Error {
    fn from(_: Refused) -> Self {
        crate::Error::Invalid
    }
}

/// `create`'s record. Its `version` is the header's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Create {
    pub vcpus: u16,
    pub ram_mib: u32,
    pub flags: u32,
}

/// `set_regs`'s record: one vCPU's state for the Linux 64-bit boot protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    pub vcpu: u16,
    /// RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, then R8 to R15: the order of
    /// their x86 register numbers.
    pub gpr: [u64; 16],
    pub rip: u64,
    pub rflags: u64,
    pub cr0: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub efer: u64,
    pub gdt_base: u64,
    pub gdt_limit: u16,
    pub idt_base: u64,
    pub idt_limit: u16,
    /// The code selector, and the one DS, ES, SS, FS and GS share.
    pub cs: u16,
    pub ds: u16,
}

/// `irq`'s record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Irq {
    /// An IOAPIC pin driven to `level` (0 or 1).
    Pin { pin: u8, level: u8 },
    /// One message-signalled interrupt.
    Msi { address: u64, data: u32 },
}

const IRQ_PIN: u8 = 1;
const IRQ_MSI: u8 = 2;

/// One `VM` call's record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Create(Create),
    SetRegs(Regs),
    Run,
    Irq(Irq),
    Stop,
    Destroy,
}

impl Call {
    /// The operation, the call's RDI.
    pub const fn op(&self) -> u16 {
        match self {
            Call::Create(_) => op::CREATE,
            Call::SetRegs(_) => op::SET_REGS,
            Call::Run => op::RUN,
            Call::Irq(_) => op::IRQ,
            Call::Stop => op::STOP,
            Call::Destroy => op::DESTROY,
        }
    }

    /// The record's bytes in `out`, and how many. A record out of range is
    /// written as it is, and [`decode`] refuses it.
    pub fn encode(&self, out: &mut [u8; MAX_CALL_BYTES]) -> usize {
        *out = [0; MAX_CALL_BYTES];
        put16(out, 0, VERSION);
        put16(out, 2, self.op());
        match self {
            Call::Create(c) => {
                put16(out, 8, c.vcpus);
                put32(out, 12, c.ram_mib);
                put32(out, 16, c.flags);
                CREATE_BYTES
            }
            Call::SetRegs(r) => {
                put16(out, 8, r.vcpu);
                put16(out, 10, r.cs);
                put16(out, 12, r.ds);
                put16(out, 14, r.gdt_limit);
                put16(out, 16, r.idt_limit);
                for (i, value) in r.gpr.iter().enumerate() {
                    put64(out, 24 + 8 * i, *value);
                }
                let tail = [
                    r.rip, r.rflags, r.cr0, r.cr3, r.cr4, r.efer, r.gdt_base, r.idt_base,
                ];
                for (i, value) in tail.iter().enumerate() {
                    put64(out, 152 + 8 * i, *value);
                }
                REGS_BYTES
            }
            Call::Irq(Irq::Pin { pin, level }) => {
                out[8] = IRQ_PIN;
                out[9] = *pin;
                out[10] = *level;
                IRQ_BYTES
            }
            Call::Irq(Irq::Msi { address, data }) => {
                out[8] = IRQ_MSI;
                put32(out, 12, *data);
                put64(out, 16, *address);
                IRQ_BYTES
            }
            Call::Run | Call::Stop | Call::Destroy => BARE_BYTES,
        }
    }

    /// Whether every field is in range (the module doc's rules).
    pub fn check(&self) -> Result<(), Refused> {
        let ok = match self {
            Call::Create(c) => {
                (1..=MAX_VCPUS).contains(&c.vcpus)
                    && (RAM_STEP_MIB..=MAX_RAM_MIB).contains(&c.ram_mib)
                    && c.ram_mib.is_multiple_of(RAM_STEP_MIB)
                    && c.flags == 0
            }
            Call::SetRegs(r) => regs_ok(r),
            Call::Irq(Irq::Pin { pin, level }) => *pin < IOAPIC_PINS && *level <= 1,
            Call::Irq(Irq::Msi { address, data }) => {
                (MSI_FIRST..=MSI_LAST).contains(address) && *data <= 0xFFFF
            }
            Call::Run | Call::Stop | Call::Destroy => true,
        };
        ok.then_some(()).ok_or(Refused::Range)
    }
}

fn regs_ok(r: &Regs) -> bool {
    let bits =
        |value: u64, allowed: u64, needed: u64| value & !allowed == 0 && value & needed == needed;
    let selector = |sel: u16| {
        // A GDT selector (TI 0) at RPL 0, not the null one, inside the limit.
        sel & 7 == 0 && sel != 0 && u32::from(sel) + 7 <= u32::from(r.gdt_limit)
    };
    r.vcpu < MAX_VCPUS
        && bits(r.cr0, CR0_ALLOWED, CR0_NEEDED)
        // CR0.NW without CD is a combination AMD VMRUN refuses.
        && (r.cr0 & CR0_NW == 0 || r.cr0 & CR0_CD != 0)
        && bits(r.cr4, CR4_ALLOWED, CR4_NEEDED)
        && bits(r.efer, EFER_ALLOWED, EFER_NEEDED)
        && bits(r.rflags, RFLAGS_DEFINED, RFLAGS_ONE)
        && r.cr3 & 0xFFF == 0
        && r.cr3 < PHYS_LIMIT
        && canonical(r.rip)
        && canonical(r.gdt_base)
        && canonical(r.idt_base)
        && selector(r.cs)
        && selector(r.ds)
}

/// Whether `address` is canonical for 48-bit virtual addresses.
const fn canonical(address: u64) -> bool {
    let top = address >> 47;
    top == 0 || top == (1 << 17) - 1
}

/// The record in `bytes` for the call whose RDI said `op`: [`decode`], but a
/// header whose operation is not `op` is refused with [`Refused::Op`], so the
/// two copies of the operation cannot disagree.
pub fn decode_for(op: u16, bytes: &[u8]) -> Result<Call, Refused> {
    let call = decode(bytes)?;
    if get16(bytes, 2) != op {
        return Err(Refused::Op);
    }
    Ok(call)
}

/// The record in `bytes`, if it is one v1 record whose every field is in range.
pub fn decode(bytes: &[u8]) -> Result<Call, Refused> {
    if bytes.len() < HEADER_BYTES {
        return Err(Refused::Length);
    }
    if get16(bytes, 0) != VERSION {
        return Err(Refused::Version);
    }
    let want = match get16(bytes, 2) {
        op::CREATE => CREATE_BYTES,
        op::SET_REGS => REGS_BYTES,
        op::IRQ => IRQ_BYTES,
        op::RUN | op::STOP | op::DESTROY => BARE_BYTES,
        _ => return Err(Refused::Op),
    };
    if bytes.len() != want {
        return Err(Refused::Length);
    }
    let zero = |from: usize, to: usize| bytes[from..to].iter().all(|b| *b == 0);
    let call = match get16(bytes, 2) {
        op::CREATE => {
            if !zero(4, 8) || !zero(10, 12) || !zero(20, 24) {
                return Err(Refused::Reserved);
            }
            Call::Create(Create {
                vcpus: get16(bytes, 8),
                ram_mib: get32(bytes, 12),
                flags: get32(bytes, 16),
            })
        }
        op::SET_REGS => {
            if !zero(4, 8) || !zero(18, 24) {
                return Err(Refused::Reserved);
            }
            let at = |i: usize| get64(bytes, 152 + 8 * i);
            Call::SetRegs(Regs {
                vcpu: get16(bytes, 8),
                cs: get16(bytes, 10),
                ds: get16(bytes, 12),
                gdt_limit: get16(bytes, 14),
                idt_limit: get16(bytes, 16),
                gpr: core::array::from_fn(|i| get64(bytes, 24 + 8 * i)),
                rip: at(0),
                rflags: at(1),
                cr0: at(2),
                cr3: at(3),
                cr4: at(4),
                efer: at(5),
                gdt_base: at(6),
                idt_base: at(7),
            })
        }
        op::IRQ => {
            if !zero(4, 8) || bytes[11] != 0 {
                return Err(Refused::Reserved);
            }
            match bytes[8] {
                IRQ_PIN => {
                    if !zero(12, 24) {
                        return Err(Refused::Reserved);
                    }
                    Call::Irq(Irq::Pin {
                        pin: bytes[9],
                        level: bytes[10],
                    })
                }
                IRQ_MSI => {
                    if !zero(9, 11) {
                        return Err(Refused::Reserved);
                    }
                    Call::Irq(Irq::Msi {
                        data: get32(bytes, 12),
                        address: get64(bytes, 16),
                    })
                }
                _ => return Err(Refused::Op),
            }
        }
        op::RUN | op::STOP | op::DESTROY => {
            if !zero(4, 8) {
                return Err(Refused::Reserved);
            }
            match get16(bytes, 2) {
                op::RUN => Call::Run,
                op::STOP => Call::Stop,
                _ => Call::Destroy,
            }
        }
        _ => return Err(Refused::Op),
    };
    call.check()?;
    Ok(call)
}

// ---- the VMEXIT ring

/// What stopped a vCPU, or a doorbell it posted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// An MMIO access of `size` bytes at guest-physical `address`; `write` is
    /// the data of a write, `None` for a read.
    Mmio {
        address: u64,
        size: u8,
        write: Option<u64>,
    },
    /// A port I/O access of `size` bytes.
    Pio {
        port: u16,
        size: u8,
        write: Option<u64>,
    },
    /// A write to a virtio notify register, posted without stopping the vCPU.
    Doorbell {
        address: u64,
        size: u8,
        data: u64,
    },
    Shutdown,
    TripleFault,
}

/// One `VMEXIT` entry: the exit sequence (the request's `tag`), the vCPU and
/// the exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitEntry {
    pub seq: u64,
    pub vcpu: u16,
    pub exit: Exit,
}

impl ExitEntry {
    /// The ring request this entry is.
    pub fn request(&self) -> Request {
        let (op, offset, len, write) = match self.exit {
            Exit::Mmio {
                address,
                size,
                write,
            } => (vmexit::MMIO, address, size, write),
            Exit::Pio { port, size, write } => (vmexit::PIO, u64::from(port), size, write),
            Exit::Doorbell {
                address,
                size,
                data,
            } => (vmexit::DOORBELL, address, size, Some(data)),
            Exit::Shutdown => (vmexit::SHUTDOWN, 0, 0, None),
            Exit::TripleFault => (vmexit::TRIPLE_FAULT, 0, 0, None),
        };
        Request {
            tag: self.seq,
            op,
            flags: if write.is_some() { vmexit::WRITE } else { 0 },
            buffer: 0,
            offset,
            len: u32::from(len),
            _r: 0,
            args: [u64::from(self.vcpu), write.unwrap_or(0), 0, 0],
        }
    }

    /// The entry a peer's `request` is, if it is one under the module doc's
    /// rules.
    pub fn from_request(r: &Request) -> Result<Self, Refused> {
        if r.buffer != 0 || r._r != 0 || r.args[2] != 0 || r.args[3] != 0 {
            return Err(Refused::Reserved);
        }
        if r.flags & !vmexit::WRITE != 0 {
            return Err(Refused::Reserved);
        }
        let vcpu = u16::try_from(r.args[0])
            .ok()
            .filter(|v| *v < MAX_VCPUS)
            .ok_or(Refused::Range)?;
        let is_write = r.flags & vmexit::WRITE != 0;
        let data = r.args[1];
        let size = u8::try_from(r.len).map_err(|_| Refused::Range)?;
        // A write's data fits its width; a read carries none.
        let write = |sizes: &[u8]| -> Result<Option<u64>, Refused> {
            if !sizes.contains(&size) {
                return Err(Refused::Range);
            }
            match is_write {
                true if size < 8 && data >> (8 * u32::from(size)) != 0 => Err(Refused::Range),
                true => Ok(Some(data)),
                false if data != 0 => Err(Refused::Reserved),
                false => Ok(None),
            }
        };
        let exit = match r.op {
            vmexit::MMIO => {
                let write = write(&[1, 2, 4, 8])?;
                if r.offset > PHYS_LIMIT - u64::from(size) {
                    return Err(Refused::Range);
                }
                Exit::Mmio {
                    address: r.offset,
                    size,
                    write,
                }
            }
            vmexit::PIO => {
                let write = write(&[1, 2, 4])?;
                if r.offset > 0x1_0000 - u64::from(size) {
                    return Err(Refused::Range);
                }
                Exit::Pio {
                    port: r.offset as u16,
                    size,
                    write,
                }
            }
            vmexit::DOORBELL => {
                if !is_write {
                    return Err(Refused::Range);
                }
                let data = write(&[2, 4])?.unwrap_or(0);
                if r.offset > PHYS_LIMIT - u64::from(size) {
                    return Err(Refused::Range);
                }
                Exit::Doorbell {
                    address: r.offset,
                    size,
                    data,
                }
            }
            vmexit::SHUTDOWN | vmexit::TRIPLE_FAULT => {
                if r.offset != 0 || r.len != 0 || r.flags != 0 || data != 0 {
                    return Err(Refused::Reserved);
                }
                match r.op {
                    vmexit::SHUTDOWN => Exit::Shutdown,
                    _ => Exit::TripleFault,
                }
            }
            _ => return Err(Refused::Op),
        };
        Ok(ExitEntry {
            seq: r.tag,
            vcpu,
            exit,
        })
    }

    /// The entry in a peer's 64 raw bytes, laid out as [`Request`] is.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Refused> {
        if bytes.len() != ring::REQUEST_BYTES {
            return Err(Refused::Length);
        }
        let request = Request {
            tag: get64(bytes, 0),
            op: get16(bytes, 8),
            flags: get16(bytes, 10),
            buffer: get32(bytes, 12),
            offset: get64(bytes, 16),
            len: get32(bytes, 24),
            _r: get32(bytes, 28),
            args: core::array::from_fn(|i| get64(bytes, 32 + 8 * i)),
        };
        Self::from_request(&request)
    }

    /// The 64 bytes of [`Self::request`], laid out as [`Request`] is.
    pub fn to_bytes(&self) -> [u8; ring::REQUEST_BYTES] {
        let r = self.request();
        let mut out = [0; ring::REQUEST_BYTES];
        put64(&mut out, 0, r.tag);
        put16(&mut out, 8, r.op);
        put16(&mut out, 10, r.flags);
        put32(&mut out, 12, r.buffer);
        put64(&mut out, 16, r.offset);
        put32(&mut out, 24, r.len);
        put32(&mut out, 28, r._r);
        for (i, arg) in r.args.iter().enumerate() {
            put64(&mut out, 32 + 8 * i, *arg);
        }
        out
    }

    /// Whether `vmm` returns read data for this exit: an MMIO or port read.
    pub const fn reads(&self) -> Option<u8> {
        match self.exit {
            Exit::Mmio {
                size, write: None, ..
            }
            | Exit::Pio {
                size, write: None, ..
            } => Some(size),
            _ => None,
        }
    }
}

/// Byte offset in the ring's data area of the 8-byte slot for exit `seq`, in
/// a ring of `entries` (a power of two from 8 to 4,096) whose data area is
/// `data_len` bytes, or `None` for any other `entries` or a data area shorter
/// than `entries * 8`.
pub fn data_slot(seq: u64, entries: u32, data_len: usize) -> Option<usize> {
    let ok = entries.is_power_of_two()
        && (ring::MIN_ENTRIES..=ring::MAX_ENTRIES).contains(&entries)
        && data_len >= entries as usize * 8;
    ok.then(|| (seq & u64::from(entries - 1)) as usize * 8)
}

/// Checks `vmm`'s `reply` to `entry`, and gives the value read, if any:
/// `Ok(None)` for an answered write, a doorbell or an exit with no data, and
/// `Err` for an answer the module doc's rules refuse. `slot` is the entry's
/// data slot as read from the ring. A nonzero `status` is an answer too,
/// returned as `Ok(Err(code))`.
pub fn check_reply(
    entry: &ExitEntry,
    reply: &Response,
    slot: [u8; 8],
) -> Result<Result<Option<u64>, crate::Error>, Refused> {
    if reply.tag != entry.seq {
        return Err(Refused::Range);
    }
    if reply.status != 0 {
        let code = reply.status.checked_neg().ok_or(Refused::Range)?;
        let error = crate::Error::ALL
            .into_iter()
            .find(|e| *e as i32 == code)
            .ok_or(Refused::Range)?;
        return match reply.len {
            0 => Ok(Err(error)),
            _ => Err(Refused::Reserved),
        };
    }
    match entry.reads() {
        None if reply.len == 0 => Ok(Ok(None)),
        None => Err(Refused::Reserved),
        Some(size) if reply.len == u32::from(size) => {
            let value = u64::from_le_bytes(slot);
            if size < 8 && value >> (8 * u32::from(size)) != 0 {
                return Err(Refused::Reserved);
            }
            Ok(Ok(Some(value)))
        }
        Some(_) => Err(Refused::Length),
    }
}

fn get16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn get32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(core::array::from_fn(|i| b[at + i]))
}

fn get64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(core::array::from_fn(|i| b[at + i]))
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests;
