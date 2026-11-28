//! The codec's widget graph (HD Audio 1.0a section 7.1): from the root node to
//! the audio function group, then from an output pin back through mixers and
//! selectors to an output converter (a DAC). The walk asks the codec as it
//! goes and keeps a parent, a depth and the capabilities per node it reached,
//! so it needs no heap and under 3 KiB of stack.

use crate::Error;
use crate::verb::{
    self, AUDIO_FUNCTION_GROUP, Config, Device, Kind, PinCaps, Verb, WidgetCaps, param,
};

/// One codec's answers, by node: the controller's CORB and RIRB, or a test's.
pub trait Codec {
    fn verb(&mut self, nid: u8, verb: Verb) -> Result<u32, Error>;
}

/// The most widgets a path may pass through, the pin and the DAC included.
pub const MAX_PATH: usize = 8;
/// The most connections read from one widget's list.
pub const MAX_CONNECTIONS: usize = 32;

/// A widget's connection list, ranges expanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Connections {
    nids: [u8; MAX_CONNECTIONS],
    len: usize,
}

impl Connections {
    pub fn as_slice(&self) -> &[u8] {
        &self.nids[..self.len]
    }

    fn push(&mut self, nid: u8) -> Result<(), Error> {
        if self.len == MAX_CONNECTIONS {
            return Err(Error::Codec("a connection list longer than 32"));
        }
        self.nids[self.len] = nid;
        self.len += 1;
        Ok(())
    }
}

/// A node's connection list (section 7.3.3.3). In the short form each response
/// holds four 8-bit entries, in the long form two 16-bit ones (whose node
/// numbers this driver, with 8-bit nodes, cannot use past 255); an entry with
/// its top bit set ends a range that starts after the entry before it.
pub fn connections(codec: &mut dyn Codec, nid: u8) -> Result<Connections, Error> {
    let mut list = Connections {
        nids: [0; MAX_CONNECTIONS],
        len: 0,
    };
    let length = codec.verb(nid, Verb::get_parameter(param::CONNECTION_LIST_LENGTH))?;
    let (count, long) = verb::connection_list_length(length);
    let per = if long { 2 } else { 4 };
    let (bits, range) = if long {
        (16, 1u32 << 15)
    } else {
        (8, 1u32 << 7)
    };
    let mut previous: Option<u32> = None;
    for index in 0..count {
        if usize::from(index) >= MAX_CONNECTIONS * 2 {
            return Err(Error::Codec("a connection list longer than 32"));
        }
        let word = codec.verb(nid, Verb::get_connection_list(index - index % per))?;
        let entry = (word >> (bits * u32::from(index % per))) & ((1u32 << bits) - 1);
        let target = entry & !range;
        if target > 0xFF {
            return Err(Error::Codec("a connection to a node past 255"));
        }
        match (entry & range != 0, previous) {
            (true, Some(start)) if start < target => {
                for n in start + 1..=target {
                    list.push(n as u8)?;
                }
            }
            (true, _) => return Err(Error::Codec("a connection range with no start")),
            (false, _) => list.push(target as u8)?,
        }
        previous = Some(target);
    }
    Ok(list)
}

/// The codec's audio function group: its node and its widgets' node range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionGroup {
    pub nid: u8,
    pub first: u8,
    pub count: u8,
}

impl FunctionGroup {
    fn contains(&self, nid: u8) -> bool {
        nid >= self.first && u16::from(nid) < u16::from(self.first) + u16::from(self.count)
    }
}

/// The root node's first audio function group (a modem group is passed over).
pub fn audio_function_group(codec: &mut dyn Codec) -> Result<FunctionGroup, Error> {
    let (first, count) = verb::node_count(codec.verb(0, Verb::get_parameter(param::NODE_COUNT))?);
    for nid in first..first.saturating_add(count) {
        let kind = codec.verb(nid, Verb::get_parameter(param::FUNCTION_GROUP_TYPE))?;
        if (kind & 0xFF) as u8 == AUDIO_FUNCTION_GROUP {
            let (first, count) =
                verb::node_count(codec.verb(nid, Verb::get_parameter(param::NODE_COUNT))?);
            return Ok(FunctionGroup { nid, first, count });
        }
    }
    Err(Error::NoAudioFunctionGroup)
}

/// One step of a path: a widget and, for all but the DAC, the index in its
/// connection list of the next widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hop {
    pub nid: u8,
    pub kind: Kind,
    pub caps: WidgetCaps,
    /// Where the next widget is in this one's connection list.
    pub input: u8,
    /// How many connections this widget has.
    pub inputs: u8,
}

/// An output path: a pin first, the DAC last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Path {
    hops: [Hop; MAX_PATH],
    len: usize,
    pub pin_caps: PinCaps,
    pub device: Device,
}

impl Path {
    pub fn hops(&self) -> &[Hop] {
        &self.hops[..self.len]
    }

    pub fn pin(&self) -> u8 {
        self.hops[0].nid
    }

    pub fn dac(&self) -> u8 {
        self.hops[self.len - 1].nid
    }
}

/// The output path to play on: of the connected output-capable pins, a line
/// out first, then a speaker, then headphones, then anything else, each in
/// node order; and from that pin the shortest way through mixers and
/// selectors to a DAC (breadth first, so a direct DAC beats a mixer's).
/// Digital pins and converters are left out: the stream is analogue PCM.
pub fn output_path(codec: &mut dyn Codec, group: &FunctionGroup) -> Result<Path, Error> {
    let mut best: Option<(Device, Path)> = None;
    for nid in group.first..group.first.saturating_add(group.count) {
        let caps = WidgetCaps(codec.verb(nid, Verb::get_parameter(param::AUDIO_WIDGET_CAP))?);
        if caps.kind() != Kind::Pin || caps.digital() {
            continue;
        }
        let pin_caps = PinCaps(codec.verb(nid, Verb::get_parameter(param::PIN_CAP))?);
        let config = Config(codec.verb(nid, Verb::get_config_default())?);
        if !pin_caps.output() || !config.connected() || !caps.connection_list() {
            continue;
        }
        let device = config.device();
        if best.as_ref().is_some_and(|(d, _)| *d <= device) {
            continue;
        }
        if let Some(path) = to_dac(codec, group, nid, caps, pin_caps, device)? {
            best = Some((device, path));
        }
    }
    best.map(|(_, path)| path).ok_or(Error::NoOutputPath)
}

/// Breadth first from `pin` to the nearest analogue DAC, or None.
fn to_dac(
    codec: &mut dyn Codec,
    group: &FunctionGroup,
    pin: u8,
    pin_widget: WidgetCaps,
    pin_caps: PinCaps,
    device: Device,
) -> Result<Option<Path>, Error> {
    // parent[n] is the node that reached n, and at which index of its list;
    // depth[n] is how many hops from the pin. The queue holds each node once.
    let mut parent = [(0u8, 0u8); 256];
    let mut depth = [0u8; 256];
    let mut seen = [false; 256];
    let mut queue = [0u8; 256];
    let (mut head, mut tail) = (0, 0);
    let mut caps_of = [WidgetCaps(0); 256];
    seen[usize::from(pin)] = true;
    caps_of[usize::from(pin)] = pin_widget;
    queue[tail] = pin;
    tail += 1;
    while head < tail {
        let nid = queue[head];
        head += 1;
        let caps = caps_of[usize::from(nid)];
        let kind = caps.kind();
        if kind == Kind::Output && !caps.digital() {
            return Ok(Some(walk_back(
                codec, pin, nid, &parent, &depth, &caps_of, pin_caps, device,
            )?));
        }
        let passes = matches!(kind, Kind::Pin | Kind::Mixer | Kind::Selector);
        if !passes
            || !caps.connection_list()
            || usize::from(depth[usize::from(nid)]) + 1 >= MAX_PATH
        {
            continue;
        }
        let list = connections(codec, nid)?;
        for (index, &next) in list.as_slice().iter().enumerate() {
            let n = usize::from(next);
            if seen[n] || !group.contains(next) {
                continue;
            }
            seen[n] = true;
            caps_of[n] =
                WidgetCaps(codec.verb(next, Verb::get_parameter(param::AUDIO_WIDGET_CAP))?);
            parent[n] = (nid, index as u8);
            depth[n] = depth[usize::from(nid)] + 1;
            queue[tail] = next;
            tail += 1;
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn walk_back(
    codec: &mut dyn Codec,
    pin: u8,
    dac: u8,
    parent: &[(u8, u8); 256],
    depth: &[u8; 256],
    caps_of: &[WidgetCaps; 256],
    pin_caps: PinCaps,
    device: Device,
) -> Result<Path, Error> {
    let len = usize::from(depth[usize::from(dac)]) + 1;
    let blank = Hop {
        nid: 0,
        kind: Kind::Output,
        caps: WidgetCaps(0),
        input: 0,
        inputs: 0,
    };
    let mut hops = [blank; MAX_PATH];
    let mut nid = dac;
    let mut input = 0;
    for slot in (0..len).rev() {
        let caps = caps_of[usize::from(nid)];
        let inputs = match caps.connection_list() {
            true => connections(codec, nid)?.as_slice().len() as u8,
            false => 0,
        };
        hops[slot] = Hop {
            nid,
            kind: caps.kind(),
            caps,
            input,
            inputs,
        };
        if nid == pin {
            break;
        }
        let (up, index) = parent[usize::from(nid)];
        input = index;
        nid = up;
    }
    Ok(Path {
        hops,
        len,
        pin_caps,
        device,
    })
}
