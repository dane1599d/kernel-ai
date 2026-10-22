//! Transfer Request Blocks: the 16-byte entries of every ring. The driver builds
//! commands and transfers here and reads events back.

/// One TRB as four dwords: parameter low and high, status, control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trb(pub [u32; 4]);

pub const BYTES: usize = 16;

// TRB types (control bits 15:10).
pub const NORMAL: u8 = 1;
pub const SETUP: u8 = 2;
pub const DATA: u8 = 3;
pub const STATUS: u8 = 4;
pub const LINK: u8 = 6;
pub const ENABLE_SLOT: u8 = 9;
pub const ADDRESS_DEVICE: u8 = 11;
pub const CONFIGURE_ENDPOINT: u8 = 12;
pub const EVALUATE_CONTEXT: u8 = 13;
pub const RESET_ENDPOINT: u8 = 14;
pub const SET_DEQUEUE: u8 = 16;
pub const NOOP_COMMAND: u8 = 23;
pub const TRANSFER_EVENT: u8 = 32;
pub const COMMAND_COMPLETION: u8 = 33;
pub const PORT_STATUS_CHANGE: u8 = 34;

// Control bits.
const CYCLE: u32 = 1;
/// Link TRB: toggle the cycle state on following it.
const TOGGLE_CYCLE: u32 = 1 << 1;
/// Normal TRB: an event on a short packet too.
const SHORT_PACKET_EVENT: u32 = 1 << 2;
const INTERRUPT_ON_COMPLETION: u32 = 1 << 5;
/// Setup TRB: the eight bytes are in the TRB itself.
const IMMEDIATE_DATA: u32 = 1 << 6;
/// Data and status stages: the stage runs IN, device to host.
const DIRECTION_IN: u32 = 1 << 16;

// Completion codes.
pub const SUCCESS: u8 = 1;
pub const STALL: u8 = 6;
pub const SHORT_PACKET: u8 = 13;

/// A text for a completion code, for log lines.
pub fn completion_text(code: u8) -> &'static str {
    match code {
        SUCCESS => "success",
        2 => "data buffer error",
        3 => "babble detected",
        4 => "USB transaction error",
        5 => "TRB error",
        STALL => "stall",
        7 => "resource error",
        8 => "bandwidth error",
        9 => "no slots available",
        11 => "slot not enabled",
        12 => "endpoint not enabled",
        SHORT_PACKET => "short packet",
        17 => "parameter error",
        19 => "context state error",
        _ => "an unexpected completion code",
    }
}

fn control(kind: u8) -> u32 {
    u32::from(kind) << 10
}

fn low(address: u64) -> u32 {
    address as u32
}

fn high(address: u64) -> u32 {
    (address >> 32) as u32
}

impl Trb {
    pub fn kind(&self) -> u8 {
        ((self.0[3] >> 10) & 0x3F) as u8
    }

    pub fn cycle(&self) -> bool {
        self.0[3] & CYCLE != 0
    }

    /// The same TRB with its cycle bit set to `cycle`.
    pub fn with_cycle(mut self, cycle: bool) -> Self {
        self.0[3] = (self.0[3] & !CYCLE) | u32::from(cycle);
        self
    }

    /// The 64-bit parameter: a pointer in most TRBs.
    pub fn parameter(&self) -> u64 {
        u64::from(self.0[0]) | (u64::from(self.0[1]) << 32)
    }

    /// The link back to the ring's start, flipping the cycle state.
    pub fn link(ring: u64) -> Self {
        Self([low(ring), high(ring), 0, control(LINK) | TOGGLE_CYCLE])
    }

    pub fn noop_command() -> Self {
        Self([0, 0, 0, control(NOOP_COMMAND)])
    }

    pub fn enable_slot() -> Self {
        Self([0, 0, 0, control(ENABLE_SLOT)])
    }

    /// A command on `slot` with the input context at `input`.
    fn with_input(kind: u8, input: u64, slot: u8) -> Self {
        Self([
            low(input),
            high(input),
            0,
            control(kind) | (u32::from(slot) << 24),
        ])
    }

    pub fn address_device(input: u64, slot: u8) -> Self {
        Self::with_input(ADDRESS_DEVICE, input, slot)
    }

    pub fn configure_endpoint(input: u64, slot: u8) -> Self {
        Self::with_input(CONFIGURE_ENDPOINT, input, slot)
    }

    pub fn evaluate_context(input: u64, slot: u8) -> Self {
        Self::with_input(EVALUATE_CONTEXT, input, slot)
    }

    /// Resets endpoint `dci` of `slot` out of its halted state.
    pub fn reset_endpoint(slot: u8, dci: u8) -> Self {
        Self([
            0,
            0,
            0,
            control(RESET_ENDPOINT) | (u32::from(dci) << 16) | (u32::from(slot) << 24),
        ])
    }

    /// Points endpoint `dci` of `slot` at `dequeue` with cycle state `cycle`.
    pub fn set_dequeue(slot: u8, dci: u8, dequeue: u64, cycle: bool) -> Self {
        let pointer = dequeue | u64::from(cycle);
        Self([
            low(pointer),
            high(pointer),
            0,
            control(SET_DEQUEUE) | (u32::from(dci) << 16) | (u32::from(slot) << 24),
        ])
    }

    /// The setup stage of a control transfer, with the 8-byte request inline.
    /// `data` says the direction of a data stage, if one follows.
    pub fn setup(request: [u8; 8], data: Option<Direction>) -> Self {
        let transfer_type = match data {
            None => 0,
            Some(Direction::Out) => 2,
            Some(Direction::In) => 3,
        };
        Self([
            u32::from_le_bytes([request[0], request[1], request[2], request[3]]),
            u32::from_le_bytes([request[4], request[5], request[6], request[7]]),
            8,
            control(SETUP) | IMMEDIATE_DATA | (transfer_type << 16),
        ])
    }

    /// The data stage: `len` bytes at `buffer`. A short packet raises an event,
    /// which says how many bytes came.
    pub fn data(buffer: u64, len: u16, direction: Direction) -> Self {
        let dir = if direction == Direction::In {
            DIRECTION_IN
        } else {
            0
        };
        Self([
            low(buffer),
            high(buffer),
            u32::from(len),
            control(DATA) | dir | SHORT_PACKET_EVENT,
        ])
    }

    /// The status stage, which runs the other way from the data stage (IN when
    /// there was none) and raises the transfer's event.
    pub fn status(data: Option<Direction>) -> Self {
        let dir = match data {
            Some(Direction::In) => 0,
            _ => DIRECTION_IN,
        };
        Self([0, 0, 0, control(STATUS) | dir | INTERRUPT_ON_COMPLETION])
    }

    /// One interrupt-IN (or bulk) transfer of up to `len` bytes into `buffer`.
    pub fn normal(buffer: u64, len: u16) -> Self {
        Self([
            low(buffer),
            high(buffer),
            u32::from(len),
            control(NORMAL) | INTERRUPT_ON_COMPLETION | SHORT_PACKET_EVENT,
        ])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

/// An event the controller wrote to the event ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A transfer finished: the TRB it was for, what the TRB asked for but did
    /// not move, the code, and where.
    Transfer {
        trb: u64,
        residue: u32,
        code: u8,
        slot: u8,
        dci: u8,
    },
    /// A command finished: the command TRB, the code, and the slot it names
    /// (Enable Slot's new slot).
    Command {
        trb: u64,
        code: u8,
        slot: u8,
    },
    PortStatus {
        port: u8,
    },
    Other {
        kind: u8,
    },
}

impl Event {
    pub fn parse(trb: Trb) -> Self {
        let code = (trb.0[2] >> 24) as u8;
        let slot = (trb.0[3] >> 24) as u8;
        match trb.kind() {
            TRANSFER_EVENT => Event::Transfer {
                trb: trb.parameter(),
                residue: trb.0[2] & 0xFF_FFFF,
                code,
                slot,
                dci: ((trb.0[3] >> 16) & 0x1F) as u8,
            },
            COMMAND_COMPLETION => Event::Command {
                trb: trb.parameter(),
                code,
                slot,
            },
            PORT_STATUS_CHANGE => Event::PortStatus {
                port: (trb.0[0] >> 24) as u8,
            },
            kind => Event::Other { kind },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_trbs_point_back_and_toggle_the_cycle() {
        let link = Trb::link(0x1_2345_6000);
        assert_eq!(link.kind(), LINK);
        assert_eq!(link.parameter(), 0x1_2345_6000);
        assert_eq!(link.0[3] & TOGGLE_CYCLE, TOGGLE_CYCLE);
        assert!(!link.cycle());
        assert!(link.with_cycle(true).cycle());
        assert!(!link.with_cycle(true).with_cycle(false).cycle());
    }

    #[test]
    fn commands_carry_their_slot_and_input_context() {
        let address = Trb::address_device(0xABC0, 3);
        assert_eq!(address.kind(), ADDRESS_DEVICE);
        assert_eq!(address.parameter(), 0xABC0);
        assert_eq!(address.0[3] >> 24, 3);
        assert_eq!(Trb::configure_endpoint(0, 5).kind(), CONFIGURE_ENDPOINT);
        assert_eq!(Trb::evaluate_context(0, 5).kind(), EVALUATE_CONTEXT);
        assert_eq!(Trb::enable_slot().kind(), ENABLE_SLOT);
        let reset = Trb::reset_endpoint(2, 1);
        assert_eq!((reset.0[3] >> 16) & 0x1F, 1);
        assert_eq!(reset.0[3] >> 24, 2);
        let dequeue = Trb::set_dequeue(2, 1, 0x5040, true);
        assert_eq!(dequeue.parameter(), 0x5041);
    }

    #[test]
    fn a_setup_stage_holds_the_request_inline() {
        // GET_DESCRIPTOR(device, 18).
        let setup = Trb::setup([0x80, 6, 0, 1, 0, 0, 18, 0], Some(Direction::In));
        assert_eq!(setup.0[0], 0x0100_0680);
        assert_eq!(setup.0[1], 0x0012_0000);
        assert_eq!(setup.0[2], 8);
        assert_eq!(setup.kind(), SETUP);
        assert_eq!(setup.0[3] & IMMEDIATE_DATA, IMMEDIATE_DATA);
        assert_eq!((setup.0[3] >> 16) & 3, 3);
        let no_data = Trb::setup([0, 9, 1, 0, 0, 0, 0, 0], None);
        assert_eq!((no_data.0[3] >> 16) & 3, 0);
    }

    #[test]
    fn the_status_stage_runs_against_the_data() {
        assert_eq!(Trb::status(Some(Direction::In)).0[3] & DIRECTION_IN, 0);
        assert_eq!(Trb::status(None).0[3] & DIRECTION_IN, DIRECTION_IN);
        assert_eq!(
            Trb::status(Some(Direction::Out)).0[3] & DIRECTION_IN,
            DIRECTION_IN
        );
        let data = Trb::data(0x8000, 18, Direction::In);
        assert_eq!(data.0[2], 18);
        assert_eq!(data.0[3] & DIRECTION_IN, DIRECTION_IN);
        assert_eq!(data.0[3] & SHORT_PACKET_EVENT, SHORT_PACKET_EVENT);
        let normal = Trb::normal(0x9000, 8);
        assert_eq!(normal.kind(), NORMAL);
        assert_eq!(
            normal.0[3] & INTERRUPT_ON_COMPLETION,
            INTERRUPT_ON_COMPLETION
        );
    }

    #[test]
    fn events_parse_into_their_fields() {
        let transfer = Trb([
            0x7000,
            0,
            (13 << 24) | 5,
            1 | (32 << 10) | (3 << 16) | (2 << 24),
        ]);
        assert_eq!(
            Event::parse(transfer),
            Event::Transfer {
                trb: 0x7000,
                residue: 5,
                code: SHORT_PACKET,
                slot: 2,
                dci: 3
            }
        );
        let command = Trb([0x6010, 0, 1 << 24, 1 | (33 << 10) | (1 << 24)]);
        assert_eq!(
            Event::parse(command),
            Event::Command {
                trb: 0x6010,
                code: SUCCESS,
                slot: 1
            }
        );
        let port = Trb([4 << 24, 0, 1 << 24, 1 | (34 << 10)]);
        assert_eq!(Event::parse(port), Event::PortStatus { port: 4 });
        assert_eq!(
            Event::parse(Trb([0, 0, 0, 39 << 10])),
            Event::Other { kind: 39 }
        );
    }
}
