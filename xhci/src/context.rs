//! Slot and endpoint contexts, and the input context the driver hands the
//! controller with Address Device, Evaluate Context and Configure Endpoint.
//! Each context's fields fit in its first eight dwords; with CSZ set a context is
//! 64 bytes and the rest is padding.

/// The eight meaningful dwords of one context.
pub type Dwords = [u32; 8];

/// Endpoint types in an endpoint context.
pub const CONTROL: u8 = 4;
pub const INTERRUPT_IN: u8 = 7;

/// Byte offset of the context for device context index `dci` (0 is the slot)
/// inside an input context, which starts with the input control context.
pub fn input_offset(dci: u8, context_bytes: usize) -> usize {
    (usize::from(dci) + 1) * context_bytes
}

/// Byte offset of `dci`'s context in the output device context.
pub fn output_offset(dci: u8, context_bytes: usize) -> usize {
    usize::from(dci) * context_bytes
}

/// The Device Context Index of an endpoint: 1 for the control endpoint,
/// `2n + 1` for IN endpoint n and `2n` for OUT endpoint n.
pub fn dci(endpoint_address: u8) -> u8 {
    let number = endpoint_address & 0xF;
    match number {
        0 => 1,
        _ => number * 2 + u8::from(endpoint_address & 0x80 != 0),
    }
}

/// The input control context: which contexts the command adds (bit `dci`;
/// bit 0 is the slot).
pub fn input_control(add: u32) -> Dwords {
    [0, add, 0, 0, 0, 0, 0, 0]
}

/// A slot context for a device on root port `port` at `speed` whose highest
/// context is `last_dci`.
pub fn slot(speed: u8, port: u8, last_dci: u8) -> Dwords {
    let mut slot = [0; 8];
    slot[0] = (u32::from(speed) << 20) | (u32::from(last_dci) << 27);
    slot[1] = u32::from(port) << 16;
    slot
}

/// The device address the controller gave the slot, from its output slot
/// context's fourth dword.
pub fn device_address(slot_dword3: u32) -> u8 {
    slot_dword3 as u8
}

/// What an endpoint context says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub kind: u8,
    pub max_packet: u16,
    /// The service interval as 125 us times 2 to this power; 0 for control.
    pub interval: u8,
    /// The transfer ring's physical address.
    pub ring: u64,
    /// The ring's first cycle state.
    pub cycle: bool,
}

impl Endpoint {
    pub fn control(max_packet: u16, ring: u64) -> Self {
        Self {
            kind: CONTROL,
            max_packet,
            interval: 0,
            ring,
            cycle: true,
        }
    }

    pub fn interrupt_in(max_packet: u16, interval: u8, ring: u64) -> Self {
        Self {
            kind: INTERRUPT_IN,
            max_packet,
            interval,
            ring,
            cycle: true,
        }
    }

    pub fn dwords(&self) -> Dwords {
        // Three retries on errors (CErr), as every driver does.
        const ERROR_COUNT: u32 = 3 << 1;
        let mut ep = [0; 8];
        let periodic = self.kind != CONTROL;
        // Max ESIT Payload: one packet per interval for an interrupt endpoint.
        let esit = if periodic {
            u32::from(self.max_packet)
        } else {
            0
        };
        ep[0] = u32::from(self.interval) << 16;
        ep[1] = ERROR_COUNT | (u32::from(self.kind) << 3) | (u32::from(self.max_packet) << 16);
        let pointer = self.ring | u64::from(self.cycle);
        ep[2] = pointer as u32;
        ep[3] = (pointer >> 32) as u32;
        // Average TRB length: 8 bytes for control, a packet for interrupt.
        let average = if periodic { esit } else { 8 };
        ep[4] = average | (esit << 16);
        ep
    }
}

/// The endpoint context's Interval for an interrupt endpoint's `bInterval` at
/// `speed`: full and low speed give milliseconds (1 to 255), high and super
/// speed give 2 to the power `bInterval - 1` microframes.
pub fn interrupt_interval(speed: u8, b_interval: u8) -> u8 {
    use crate::regs::speed::{FULL, LOW};
    match speed {
        FULL | LOW => {
            // Eight 125 us units per millisecond; the largest power of two that
            // does not exceed the interval, between 1 ms (3) and 128 ms (10).
            let frames = u32::from(b_interval.max(1)) * 8;
            (31 - frames.leading_zeros()).clamp(3, 10) as u8
        }
        _ => b_interval.clamp(1, 16) - 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::speed;

    #[test]
    fn device_context_indexes_follow_the_endpoint_address() {
        assert_eq!(dci(0x00), 1);
        assert_eq!(dci(0x81), 3);
        assert_eq!(dci(0x01), 2);
        assert_eq!(dci(0x82), 5);
        assert_eq!(dci(0x8F), 31);
    }

    #[test]
    fn contexts_sit_at_their_index_in_both_sizes() {
        assert_eq!(input_offset(0, 32), 32);
        assert_eq!(input_offset(1, 32), 64);
        assert_eq!(input_offset(3, 64), 256);
        assert_eq!(output_offset(0, 32), 0);
        assert_eq!(output_offset(3, 32), 96);
    }

    #[test]
    fn slot_contexts_carry_speed_port_and_entries() {
        let slot = slot(speed::FULL, 5, 3);
        assert_eq!(slot[0], (1 << 20) | (3 << 27));
        assert_eq!(slot[1], 5 << 16);
        assert_eq!(device_address(0x0800_0002), 2);
    }

    #[test]
    fn a_control_endpoint_context() {
        let ep = Endpoint::control(8, 0x4000).dwords();
        assert_eq!(ep[0], 0);
        assert_eq!(ep[1], (3 << 1) | (4 << 3) | (8 << 16));
        assert_eq!(ep[2], 0x4001);
        assert_eq!(ep[3], 0);
        assert_eq!(ep[4], 8);
    }

    #[test]
    fn an_interrupt_endpoint_context() {
        let ep = Endpoint::interrupt_in(4, 6, 0x1_0000_5000).dwords();
        assert_eq!(ep[0], 6 << 16);
        assert_eq!(ep[1], (3 << 1) | (7 << 3) | (4 << 16));
        assert_eq!(ep[2], 0x5001);
        assert_eq!(ep[3], 1);
        assert_eq!(ep[4], 4 | (4 << 16));
    }

    #[test]
    fn interrupt_intervals_convert_per_speed() {
        // Full speed, 10 ms: 80 units, so 64 (2^6) = 8 ms.
        assert_eq!(interrupt_interval(speed::FULL, 10), 6);
        assert_eq!(interrupt_interval(speed::LOW, 1), 3);
        assert_eq!(interrupt_interval(speed::FULL, 255), 10);
        assert_eq!(interrupt_interval(speed::FULL, 0), 3);
        // High speed: 2^(4-1) microframes.
        assert_eq!(interrupt_interval(speed::HIGH, 4), 3);
        assert_eq!(interrupt_interval(speed::SUPER, 0), 0);
        assert_eq!(interrupt_interval(speed::HIGH, 20), 15);
    }
}
