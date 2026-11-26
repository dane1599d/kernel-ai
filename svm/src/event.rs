//! EVENTINJ (VMCB 0x0A8) and EXITINTINFO (0x088), which share one format
//! (APM vol. 2, 15.20 and 15.7.2): an event to deliver to the guest at
//! VMRUN, or the one an exit interrupted, which the hypervisor injects again.
//!
//! Bits 7:0 the vector, 10:8 the type, 11 an error code is pushed, 31 valid;
//! the error code is bits 63:32 of the 64-bit field (the control area's
//! second 32-bit word).

/// The event's type (bits 10:8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EventType {
    /// An external or virtual interrupt (INTR).
    Interrupt = 0,
    /// A non-maskable interrupt; its vector is ignored.
    Nmi = 2,
    /// An exception (a fault or trap).
    Exception = 3,
    /// A software interrupt (INTn).
    Software = 4,
}

const VALID: u32 = 1 << 31;
const ERROR_VALID: u32 = 1 << 11;

/// The exceptions that push an error code: #DF, #TS, #NP, #SS, #GP, #PF,
/// #AC, #CP, #VC and #SX.
pub const WITH_ERROR_CODE: [u8; 10] = [8, 10, 11, 12, 13, 14, 17, 21, 29, 30];

/// Exception vectors the CPU defines no exception for, or that are not one:
/// 2 is the NMI ([`Event::nmi`]), and 9, 15, 22-27 and 31 are reserved.
pub const NOT_EXCEPTIONS: [u8; 10] = [2, 9, 15, 22, 23, 24, 25, 26, 27, 31];

/// Why an event cannot be injected as asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// An exception's vector is 0-31.
    NotAnException(u8),
    /// Vector 2 (the NMI, which has its own type) or a reserved one
    /// ([`NOT_EXCEPTIONS`]).
    ReservedVector(u8),
    /// The exception pushes an error code and none was given, or the other
    /// way round.
    ErrorCode(u8),
    /// The 3-bit type is one the APM reserves (1, 5, 6 or 7).
    ReservedType(u8),
}

/// One event, valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub vector: u8,
    pub kind: EventType,
    pub error_code: Option<u32>,
}

impl Event {
    /// An external interrupt with `vector`.
    pub const fn interrupt(vector: u8) -> Self {
        Self {
            vector,
            kind: EventType::Interrupt,
            error_code: None,
        }
    }

    /// An NMI (vector 2, which the CPU ignores for this type).
    pub const fn nmi() -> Self {
        Self {
            vector: 2,
            kind: EventType::Nmi,
            error_code: None,
        }
    }

    /// A software interrupt INTn with `vector`.
    pub const fn software(vector: u8) -> Self {
        Self {
            vector,
            kind: EventType::Software,
            error_code: None,
        }
    }

    /// Exception `vector`, with the error code it pushes: refused when the
    /// vector is not an exception's (above 31, the NMI's or a reserved one),
    /// or the code is missing or extra.
    pub const fn exception(vector: u8, error_code: Option<u32>) -> Result<Self, Refusal> {
        if vector > 31 {
            return Err(Refusal::NotAnException(vector));
        }
        let mut i = 0;
        while i < NOT_EXCEPTIONS.len() {
            if NOT_EXCEPTIONS[i] == vector {
                return Err(Refusal::ReservedVector(vector));
            }
            i += 1;
        }
        let pushes = pushes_error_code(vector);
        if pushes != error_code.is_some() {
            return Err(Refusal::ErrorCode(vector));
        }
        Ok(Self {
            vector,
            kind: EventType::Exception,
            error_code,
        })
    }

    /// The 64-bit field: bits 31:0 as EVENTINJ holds them, the error code in
    /// 63:32.
    pub const fn encode(self) -> u64 {
        let mut info = self.vector as u32 | (self.kind as u32) << 8 | VALID;
        let error = match self.error_code {
            Some(code) => {
                info |= ERROR_VALID;
                code
            }
            None => 0,
        };
        info as u64 | (error as u64) << 32
    }

    /// Reads EVENTINJ or EXITINTINFO: None when its valid bit is clear (no
    /// event), an error for a reserved type.
    pub const fn decode(field: u64) -> Result<Option<Self>, Refusal> {
        let info = field as u32;
        if info & VALID == 0 {
            return Ok(None);
        }
        let kind = match (info >> 8) & 0b111 {
            0 => EventType::Interrupt,
            2 => EventType::Nmi,
            3 => EventType::Exception,
            4 => EventType::Software,
            other => return Err(Refusal::ReservedType(other as u8)),
        };
        let error_code = match info & ERROR_VALID != 0 {
            true => Some((field >> 32) as u32),
            false => None,
        };
        Ok(Some(Self {
            vector: info as u8,
            kind,
            error_code,
        }))
    }

    /// Writes the event into the control area's EVENTINJ, for the next VMRUN.
    pub fn inject(self, control: &mut crate::ControlArea) {
        let field = self.encode();
        control.event_inj = field as u32;
        control.event_inj_error = (field >> 32) as u32;
    }

    /// The event the last exit interrupted (EXITINTINFO), if any: the
    /// hypervisor injects it again so the guest does not lose it.
    pub const fn interrupted(control: &crate::ControlArea) -> Result<Option<Self>, Refusal> {
        Self::decode(control.exit_int_info as u64 | (control.exit_int_info_error as u64) << 32)
    }
}

/// Whether exception `vector` pushes an error code.
pub const fn pushes_error_code(vector: u8) -> bool {
    let mut i = 0;
    while i < WITH_ERROR_CODE.len() {
        if WITH_ERROR_CODE[i] == vector {
            return true;
        }
        i += 1;
    }
    false
}
