//! The interruption-information format (SDM vol. 3C, 25.8.3 and 28.2.2-3):
//! the VM-entry interruption-information field (VMCS 0x4016) injects an
//! event at VM entry; the VM-exit interruption information (0x4404) says
//! which event caused an exit, and the IDT-vectoring information (0x4408)
//! which event's delivery an exit interrupted, for the hypervisor to inject
//! again.
//!
//! Bits 7:0 the vector, 10:8 the type, 11 an error code is delivered, 12
//! (exit fields only) NMI unblocking due to IRET, 31 valid. The other bits
//! are 0. The error code is its own 32-bit field.

/// The event's type (bits 10:8). Type 1 is reserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EventType {
    ExternalInterrupt = 0,
    /// A non-maskable interrupt; its vector is 2.
    Nmi = 2,
    /// An exception the CPU raised (a fault, trap or abort; not INT3 or INTO).
    HardwareException = 3,
    /// INTn.
    SoftwareInterrupt = 4,
    /// INT1 (ICEBP).
    PrivilegedSoftwareException = 5,
    /// INT3 or INTO.
    SoftwareException = 6,
    /// Other: a pending MTF exit, vector 0.
    Other = 7,
}

impl EventType {
    pub const fn from_raw(raw: u8) -> Result<Self, Refusal> {
        Ok(match raw {
            0 => Self::ExternalInterrupt,
            2 => Self::Nmi,
            3 => Self::HardwareException,
            4 => Self::SoftwareInterrupt,
            5 => Self::PrivilegedSoftwareException,
            6 => Self::SoftwareException,
            7 => Self::Other,
            _ => return Err(Refusal::ReservedType(raw)),
        })
    }

    /// Whether VM entry takes an instruction length for it (VMCS 0x401A),
    /// the guest's RIP advancing past the INTn, INT1, INT3 or INTO.
    pub const fn has_length(self) -> bool {
        matches!(
            self,
            Self::SoftwareInterrupt | Self::PrivilegedSoftwareException | Self::SoftwareException
        )
    }
}

const VALID: u32 = 1 << 31;
const ERROR_CODE: u32 = 1 << 11;
const NMI_UNBLOCKING: u32 = 1 << 12;

/// The exceptions that deliver an error code, as VM entry checks it: #DF,
/// #TS, #NP, #SS, #GP, #PF, #AC and #CP.
pub const WITH_ERROR_CODE: [u8; 8] = [8, 10, 11, 12, 13, 14, 17, 21];

/// Why an event cannot be injected as asked, or an information field read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The 3-bit type is 1, which the SDM reserves.
    ReservedType(u8),
    /// The vector does not fit the type: an NMI's is not 2, a hardware
    /// exception's is past 31, an other event's is not 0.
    Vector(u8),
    /// An error code where the vector delivers none, or none where it does.
    ErrorCode(u8),
    /// A software event's instruction length is not 1-15.
    Length(u8),
    /// Reserved bits are set in an information field.
    Reserved(u32),
}

/// One event to inject at VM entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub vector: u8,
    pub kind: EventType,
    pub error_code: Option<u32>,
    /// The instruction's length for a software event; 0 otherwise.
    pub length: u8,
}

impl Event {
    pub const fn interrupt(vector: u8) -> Self {
        Self {
            vector,
            kind: EventType::ExternalInterrupt,
            error_code: None,
            length: 0,
        }
    }

    pub const fn nmi() -> Self {
        Self {
            vector: 2,
            kind: EventType::Nmi,
            error_code: None,
            length: 0,
        }
    }

    /// A hardware exception, with the error code it delivers if any.
    /// Checked as VM entry would ([`Event::check`]).
    pub const fn exception(vector: u8, error_code: Option<u32>) -> Result<Self, Refusal> {
        let event = Self {
            vector,
            kind: EventType::HardwareException,
            error_code,
            length: 0,
        };
        match event.check() {
            Ok(()) => Ok(event),
            Err(e) => Err(e),
        }
    }

    /// The checks VM entry makes of the injection field (26.2.1.3), so a
    /// bad event is refused here rather than failing the entry.
    pub const fn check(&self) -> Result<(), Refusal> {
        let v = self.vector;
        match self.kind {
            EventType::Nmi if v != 2 => return Err(Refusal::Vector(v)),
            EventType::HardwareException if v > 31 => return Err(Refusal::Vector(v)),
            EventType::Other if v != 0 => return Err(Refusal::Vector(v)),
            _ => {}
        }
        let wants_code = matches!(self.kind, EventType::HardwareException) && delivers_code(v);
        if wants_code != self.error_code.is_some() {
            return Err(Refusal::ErrorCode(v));
        }
        if self.kind.has_length() && !(self.length >= 1 && self.length <= 15) {
            return Err(Refusal::Length(self.length));
        }
        Ok(())
    }

    /// The VM-entry interruption-information field. The error code and
    /// length go in VMCS 0x4018 and 0x401A.
    pub const fn encode(&self) -> u32 {
        VALID
            | ((self.error_code.is_some() as u32) * ERROR_CODE)
            | (self.kind as u32) << 8
            | self.vector as u32
    }
}

const fn delivers_code(vector: u8) -> bool {
    let mut i = 0;
    while i < WITH_ERROR_CODE.len() {
        if WITH_ERROR_CODE[i] == vector {
            return true;
        }
        i += 1;
    }
    false
}

/// An exit's interruption information or IDT-vectoring information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitEvent {
    pub vector: u8,
    pub kind: EventType,
    /// An error code was delivered, in VMCS 0x4406 (or 0x440A).
    pub has_error_code: bool,
    /// NMI unblocking by IRET (exit interruption information only).
    pub nmi_unblocking: bool,
}

impl ExitEvent {
    /// None when the valid bit is clear: no event. Err for a reserved type
    /// or reserved bits.
    pub const fn decode(raw: u32) -> Result<Option<Self>, Refusal> {
        if raw & VALID == 0 {
            return Ok(None);
        }
        if raw & !(VALID | NMI_UNBLOCKING | ERROR_CODE | 0x7FF) != 0 {
            return Err(Refusal::Reserved(raw));
        }
        let kind = match EventType::from_raw(((raw >> 8) & 0b111) as u8) {
            Ok(kind) => kind,
            Err(e) => return Err(e),
        };
        Ok(Some(Self {
            vector: raw as u8,
            kind,
            has_error_code: raw & ERROR_CODE != 0,
            nmi_unblocking: raw & NMI_UNBLOCKING != 0,
        }))
    }
}
