//! Codec commands and responses (HD Audio 1.0a section 7.3): a command is the
//! codec address, the node and a verb with its payload, one 32-bit CORB entry;
//! a response is one 64-bit RIRB entry.

/// A verb with its payload: 12 bits of verb and 8 of payload, or 4 bits of
/// verb and 16 of payload (the converter format and amplifier verbs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verb(u32);

impl Verb {
    /// A 12-bit verb with an 8-bit payload.
    pub const fn long(verb: u16, payload: u8) -> Verb {
        Verb(((verb as u32 & 0xFFF) << 8) | payload as u32)
    }

    /// A 4-bit verb with a 16-bit payload.
    pub const fn short(verb: u8, payload: u16) -> Verb {
        Verb(((verb as u32 & 0xF) << 16) | payload as u32)
    }

    /// The 20 bits after the node in a command.
    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn get_parameter(param: u8) -> Verb {
        Verb::long(0xF00, param)
    }

    /// Four (or two, long form) connection list entries from `index`.
    pub const fn get_connection_list(index: u8) -> Verb {
        Verb::long(0xF02, index)
    }

    pub const fn set_connection_select(index: u8) -> Verb {
        Verb::long(0x701, index)
    }

    /// D0, fully on, is 0.
    pub const fn set_power_state(state: u8) -> Verb {
        Verb::long(0x705, state)
    }

    /// A converter's stream (1 to 15; 0 is none) and its lowest channel.
    pub const fn set_stream_channel(stream: u8, channel: u8) -> Verb {
        Verb::long(0x706, ((stream & 0xF) << 4) | (channel & 0xF))
    }

    pub const fn set_pin_control(control: u8) -> Verb {
        Verb::long(0x707, control)
    }

    pub const fn set_eapd(value: u8) -> Verb {
        Verb::long(0x70C, value)
    }

    pub const fn get_config_default() -> Verb {
        Verb::long(0xF1C, 0)
    }

    /// A converter's format, the same 16 bits as a stream descriptor's FMT.
    pub const fn set_converter_format(format: u16) -> Verb {
        Verb::short(0x2, format)
    }

    /// Gain and mute for the output amplifier, or input amplifier `index`, on
    /// both channels.
    pub const fn set_amp(output: bool, index: u8, mute: bool, gain: u8) -> Verb {
        let side = if output { 1 << 15 } else { 1 << 14 };
        let both = (1 << 13) | (1 << 12);
        let mute = if mute { 1 << 7 } else { 0 };
        Verb::short(
            0x3,
            side | both | (((index & 0xF) as u16) << 8) | mute | (gain & 0x7F) as u16,
        )
    }
}

/// One CORB entry: codec address, node, verb.
pub const fn command(codec: u8, nid: u8, verb: Verb) -> u32 {
    ((codec as u32 & 0xF) << 28) | ((nid as u32) << 20) | verb.bits()
}

/// GET_PARAMETER's parameters (section 7.3.4).
pub mod param {
    pub const VENDOR_ID: u8 = 0x00;
    pub const REVISION_ID: u8 = 0x02;
    pub const NODE_COUNT: u8 = 0x04;
    pub const FUNCTION_GROUP_TYPE: u8 = 0x05;
    pub const AUDIO_WIDGET_CAP: u8 = 0x09;
    pub const PCM: u8 = 0x0A;
    pub const STREAM_FORMATS: u8 = 0x0B;
    pub const PIN_CAP: u8 = 0x0C;
    pub const IN_AMP_CAP: u8 = 0x0D;
    pub const CONNECTION_LIST_LENGTH: u8 = 0x0E;
    pub const OUT_AMP_CAP: u8 = 0x12;
}

/// A NODE_COUNT response: the first subordinate node and how many.
pub fn node_count(response: u32) -> (u8, u8) {
    (((response >> 16) & 0xFF) as u8, (response & 0xFF) as u8)
}

/// FUNCTION_GROUP_TYPE: an audio function group.
pub const AUDIO_FUNCTION_GROUP: u8 = 0x01;

/// A widget's type, from AUDIO_WIDGET_CAP bits 23:20.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Output,
    Input,
    Mixer,
    Selector,
    Pin,
    Power,
    VolumeKnob,
    BeepGenerator,
    VendorDefined,
    Reserved(u8),
}

/// What AUDIO_WIDGET_CAP says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WidgetCaps(pub u32);

impl WidgetCaps {
    pub fn kind(self) -> Kind {
        match (self.0 >> 20) & 0xF {
            0x0 => Kind::Output,
            0x1 => Kind::Input,
            0x2 => Kind::Mixer,
            0x3 => Kind::Selector,
            0x4 => Kind::Pin,
            0x5 => Kind::Power,
            0x6 => Kind::VolumeKnob,
            0x7 => Kind::BeepGenerator,
            0xF => Kind::VendorDefined,
            other => Kind::Reserved(other as u8),
        }
    }

    pub fn in_amp(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    pub fn out_amp(self) -> bool {
        self.0 & (1 << 2) != 0
    }

    /// Its amplifier capabilities are its own, not the function group's.
    pub fn amp_override(self) -> bool {
        self.0 & (1 << 3) != 0
    }

    pub fn connection_list(self) -> bool {
        self.0 & (1 << 8) != 0
    }

    pub fn digital(self) -> bool {
        self.0 & (1 << 9) != 0
    }

    pub fn power_control(self) -> bool {
        self.0 & (1 << 10) != 0
    }
}

/// What PIN_CAP says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinCaps(pub u32);

impl PinCaps {
    pub fn output(self) -> bool {
        self.0 & (1 << 4) != 0
    }

    pub fn input(self) -> bool {
        self.0 & (1 << 5) != 0
    }

    pub fn headphone(self) -> bool {
        self.0 & (1 << 3) != 0
    }

    pub fn eapd(self) -> bool {
        self.0 & (1 << 16) != 0
    }
}

/// SET_PIN_CONTROL: the output is driven.
pub const PIN_OUT_ENABLE: u8 = 1 << 6;
/// SET_PIN_CONTROL: the headphone amplifier is on.
pub const PIN_HP_ENABLE: u8 = 1 << 7;
/// SET_EAPD: the external amplifier is powered.
pub const EAPD: u8 = 1 << 1;

/// A pin's configuration default (section 7.3.3.31), the board's word on what
/// the pin is wired to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config(pub u32);

/// A pin's default device, bits 23:20 of its configuration default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Device {
    LineOut,
    Speaker,
    HeadphoneOut,
    Other(u8),
}

impl Config {
    /// Bits 31:30: 1 is nothing connected.
    pub fn connected(self) -> bool {
        (self.0 >> 30) != 0b01
    }

    pub fn device(self) -> Device {
        match (self.0 >> 20) & 0xF {
            0x0 => Device::LineOut,
            0x1 => Device::Speaker,
            0x2 => Device::HeadphoneOut,
            other => Device::Other(other as u8),
        }
    }
}

/// An amplifier's capabilities (section 7.3.4.10): the step that is 0 dB.
pub fn amp_offset(caps: u32) -> u8 {
    (caps & 0x7F) as u8
}

/// The connection list's length and form, from CONNECTION_LIST_LENGTH.
pub fn connection_list_length(response: u32) -> (u8, bool) {
    ((response & 0x7F) as u8, response & (1 << 7) != 0)
}

/// One RIRB entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Response {
    pub value: u32,
    pub codec: u8,
    /// Sent by the codec on its own, not in answer to a command.
    pub unsolicited: bool,
}

impl Response {
    /// From the entry's two 32-bit halves: the response, then the codec
    /// address (bits 3:0) and the unsolicited flag (bit 4).
    pub fn from_entry(low: u32, high: u32) -> Response {
        Response {
            value: low,
            codec: (high & 0xF) as u8,
            unsolicited: high & (1 << 4) != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_put_codec_node_and_verb_where_the_spec_says() {
        assert_eq!(
            command(0, 0, Verb::get_parameter(param::VENDOR_ID)),
            0x000F_0000
        );
        assert_eq!(
            command(2, 0x1B, Verb::set_pin_control(PIN_OUT_ENABLE)),
            0x21B7_0740
        );
        assert_eq!(
            command(0, 2, Verb::set_converter_format(0x0011)),
            0x0022_0011
        );
        assert_eq!(command(0, 2, Verb::set_stream_channel(1, 0)), 0x0027_0610);
        // Output amp, both sides, unmuted, at step 0x27.
        assert_eq!(Verb::set_amp(true, 0, false, 0x27).bits(), 0x3_B027);
        // Input amp 2, both sides, muted.
        assert_eq!(Verb::set_amp(false, 2, true, 0).bits(), 0x3_7280);
        assert_eq!(command(0, 0x7F, Verb::get_config_default()), 0x07FF_1C00);
    }

    #[test]
    fn responses_and_parameters_decode() {
        assert_eq!(node_count(0x0002_0004), (2, 4));
        let r = Response::from_entry(0x1AF4_0022, 0x0000_0012);
        assert_eq!((r.value, r.codec, r.unsolicited), (0x1AF4_0022, 2, true));
        assert_eq!(WidgetCaps(0x0040_0101).kind(), Kind::Pin);
        assert_eq!(WidgetCaps(0x0020_0103).kind(), Kind::Mixer);
        assert!(WidgetCaps(0x0000_001D).out_amp());
        assert!(!WidgetCaps(0x0000_001D).connection_list());
        assert_eq!(WidgetCaps(0x00F0_0000).kind(), Kind::VendorDefined);
        assert_eq!(WidgetCaps(0x0080_0000).kind(), Kind::Reserved(8));
        assert!(PinCaps(0x0001_0010).output() && PinCaps(0x0001_0010).eapd());
        assert_eq!(connection_list_length(0x85), (5, true));
        assert!(!Config(0x4000_0000).connected());
        assert_eq!(Config(0x0121_4010).device(), Device::HeadphoneOut);
        assert_eq!(Config(0x9017_0110).device(), Device::Speaker);
        assert_eq!(amp_offset(0x8005_2727), 0x27);
    }
}
