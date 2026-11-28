//! The driver over a fake controller and codec. The fake controller keeps a
//! register file, runs the CORB into the codec when CORBWP moves, writes the
//! RIRB, and when a stream runs reads its BDL and buffer as a device would.
//! DMA memory's device address is its host address, so the fake reaches it.

use core::cell::{Cell, RefCell};
use core::ptr::NonNull;
use std::boxed::Box;
use std::collections::BTreeMap;
use std::vec;
use std::vec::Vec;

use devkit::{Dma, DmaError, Host, Mmio, Zone};

use crate::graph::{self, Codec};
use crate::regs::*;
use crate::verb::{Kind, Verb, param};
use crate::{Controller, Error, Regs};

#[repr(C, align(4096))]
struct Page([u8; 4096]);

/// Pages from the heap, at their own addresses; a clock that moves 1 µs a read.
#[derive(Default)]
struct FakeHost {
    memory: Vec<Vec<Page>>,
    zones: Vec<Zone>,
    clock: Cell<u64>,
}

impl Host for FakeHost {
    fn dma(&mut self, pages: usize, zone: Zone) -> Result<Dma, DmaError> {
        self.zones.push(zone);
        let mut block: Vec<Page> = (0..pages).map(|_| Page([0; 4096])).collect();
        let cpu = NonNull::new(block.as_mut_ptr().cast::<u8>()).unwrap();
        self.memory.push(block);
        // SAFETY: the block lives in `self.memory` (its heap buffer never
        // moves) for as long as the host, and nothing else uses it.
        Ok(unsafe { Dma::new(cpu, cpu.as_ptr() as u64, pages * 4096) })
    }
    fn mmio(&mut self, _: u8, _: u64, _: u64) -> Result<Mmio, DmaError> {
        Err(DmaError::Unsupported)
    }
    fn irqs(&mut self, _: u16) -> u16 {
        0
    }
    fn wait(&mut self, _: u64, _: u64) -> u64 {
        0
    }
    fn cfg_read(&self, _: u16) -> u32 {
        u32::MAX
    }
    fn cfg_write(&mut self, _: u16, _: u32) {}
    fn now_ns(&self) -> u64 {
        self.clock.set(self.clock.get() + 1_000);
        self.clock.get()
    }
}

/// One codec node: its parameters, configuration default and connections.
#[derive(Clone, Default)]
struct Node {
    params: BTreeMap<u8, u32>,
    config: u32,
    /// The list as the codec reports it: entries (top bit a range end).
    conns: Vec<u16>,
    long: bool,
}

/// A codec: its nodes, and every set verb it was sent, in order.
#[derive(Clone, Default)]
struct FakeCodec {
    nodes: BTreeMap<u8, Node>,
    sets: Vec<(u8, u32)>,
    /// Answers nothing.
    silent: bool,
}

impl FakeCodec {
    fn node(&mut self, nid: u8) -> &mut Node {
        self.nodes.entry(nid).or_default()
    }

    fn widget(&mut self, nid: u8, caps: u32) -> &mut Node {
        let n = self.node(nid);
        n.params.insert(param::AUDIO_WIDGET_CAP, caps);
        n
    }

    fn answer(&mut self, nid: u8, bits: u32) -> u32 {
        let node = self.nodes.get(&nid).cloned().unwrap_or_default();
        let (verb12, payload) = (bits >> 8, (bits & 0xFF) as u8);
        match verb12 {
            0xF00 => match payload {
                param::CONNECTION_LIST_LENGTH => {
                    node.conns.len() as u32 | if node.long { 0x80 } else { 0 }
                }
                p => node.params.get(&p).copied().unwrap_or(0),
            },
            0xF02 => {
                let (per, width) = if node.long { (2, 16) } else { (4, 8) };
                let start = usize::from(payload) / per * per;
                (0..per)
                    .filter_map(|i| node.conns.get(start + i).map(|&e| (i, e)))
                    .fold(0, |w, (i, e)| w | (u32::from(e) << (width * i)))
            }
            0xF1C => node.config,
            _ => {
                self.sets.push((nid, bits));
                0
            }
        }
    }

    fn sent(&self, nid: u8, verb: Verb) -> bool {
        self.sets.contains(&(nid, verb.bits()))
    }
}

/// The controller's state beyond its plain register bytes.
struct State {
    file: [u8; 0x200],
    codec: FakeCodec,
    /// The codec addresses present (STATESTS after reset).
    present: u16,
    /// The codec's address.
    address: u8,
    /// An unsolicited response goes before each answer.
    unsolicited: bool,
    /// CORBRP's reset bit reads back set until cleared (else it clears at once).
    sticky_rp_reset: bool,
    /// What a running stream played: (format, tag, the BDL's buffers' bytes).
    played: Option<(u16, u8, Vec<u8>)>,
    corb_size: u8,
    /// Commands the CORB carried.
    commands: usize,
    /// Each BDL entry's interrupt-on-completion, as the stream read them.
    ioc: Vec<bool>,
}

struct FakeRegs(RefCell<State>);

impl FakeRegs {
    fn new(codec: FakeCodec) -> FakeRegs {
        let mut file = [0u8; 0x200];
        // ICH6 as QEMU shows it: 4 input and 4 output streams, 64-bit, 1.0.
        file[0..2].copy_from_slice(&0x4401u16.to_le_bytes());
        file[VMAJ as usize] = 1;
        FakeRegs(RefCell::new(State {
            file,
            codec,
            present: 1,
            address: 0,
            unsolicited: false,
            sticky_rp_reset: true,
            played: None,
            corb_size: 0x42,
            commands: 0,
            ioc: Vec::new(),
        }))
    }

    fn state(&self) -> std::cell::RefMut<'_, State> {
        self.0.borrow_mut()
    }
}

impl State {
    fn get(&self, at: u64, n: usize) -> u64 {
        let a = at as usize;
        let mut v = 0u64;
        for i in (0..n).rev() {
            v = (v << 8) | u64::from(self.file[a + i]);
        }
        v
    }

    fn put(&mut self, at: u64, n: usize, value: u64) {
        for i in 0..n {
            self.file[at as usize + i] = (value >> (8 * i)) as u8;
        }
    }

    fn ring_base(&self, low: u64) -> *mut u8 {
        (self.get(low, 4) | (self.get(low + 4, 4) << 32)) as *mut u8
    }

    fn entries(size: u8) -> u16 {
        match size & 3 {
            0 => 2,
            1 => 16,
            _ => 256,
        }
    }

    fn respond(&mut self, value: u32, unsolicited: bool) {
        let n = Self::entries(self.get(RIRBSIZE, 1) as u8);
        let wp = ((self.get(RIRBWP, 2) as u16 + 1) % n) as u64;
        let at = unsafe { self.ring_base(RIRBLBASE).add(8 * wp as usize).cast::<u32>() };
        let high = u32::from(self.address) | if unsolicited { 1 << 4 } else { 0 };
        // SAFETY: the driver's RIRB, in a page the fake host handed out.
        unsafe {
            at.write(value);
            at.add(1).write(high);
        }
        self.put(RIRBWP, 2, wp);
        let sts = self.get(RIRBSTS, 1) | u64::from(RIRBSTS_RINTFL);
        self.put(RIRBSTS, 1, sts);
    }

    fn run_corb(&mut self) {
        if self.get(CORBCTL, 1) as u8 & RING_RUN == 0 || self.get(RIRBCTL, 1) as u8 & RING_RUN == 0
        {
            return;
        }
        let n = Self::entries(self.get(CORBSIZE, 1) as u8);
        let wp = self.get(CORBWP, 2) as u16 % n;
        let mut rp = self.get(CORBRP, 2) as u16 % n;
        while rp != wp {
            rp = (rp + 1) % n;
            // SAFETY: the driver's CORB, in a page the fake host handed out.
            let cmd = unsafe {
                self.ring_base(CORBLBASE)
                    .add(4 * rp as usize)
                    .cast::<u32>()
                    .read()
            };
            assert_eq!(
                (cmd >> 28) as u8,
                self.address,
                "a command to codec {}",
                cmd >> 28
            );
            self.commands += 1;
            if self.codec.silent {
                continue;
            }
            if self.unsolicited {
                self.respond(0xDEAD_BEEF, true);
            }
            let value = self.codec.answer((cmd >> 20) as u8, cmd & 0xF_FFFF);
            self.respond(value, false);
        }
        self.put(CORBRP, 2, u64::from(rp));
    }

    fn run_stream(&mut self, sd_base: u64) {
        let bdl =
            (self.get(sd_base + SD_BDPL, 4) | (self.get(sd_base + SD_BDPU, 4) << 32)) as *const u8;
        let lvi = self.get(sd_base + SD_LVI, 2) as usize;
        let cbl = self.get(sd_base + SD_CBL, 4) as usize;
        let mut bytes = Vec::new();
        for i in 0..=lvi {
            // SAFETY: the driver's BDL and buffers, in pages the fake host
            // handed out, read as the device would.
            unsafe {
                let e = bdl.add(16 * i);
                let addr = e.cast::<u64>().read() as *const u8;
                let len = e.add(8).cast::<u32>().read() as usize;
                self.ioc.push(e.add(12).cast::<u32>().read() & 1 != 0);
                bytes.extend_from_slice(std::slice::from_raw_parts(addr, len));
            }
        }
        assert_eq!(bytes.len(), cbl, "the BDL covers the cyclic buffer");
        let fmt = self.get(sd_base + SD_FMT, 2) as u16;
        let tag = (self.get(sd_base + SD_CTL2, 1) >> 4) as u8;
        self.played = Some((fmt, tag, bytes));
        self.put(sd_base + SD_LPIB, 4, (cbl / 2) as u64);
    }
}

impl FakeRegs {
    fn write(&self, at: u64, n: usize, value: u64) {
        let mut s = self.state();
        match at {
            GCTL => {
                let was = s.get(GCTL, 4) & 1;
                s.put(GCTL, 4, value);
                if was == 0 && value & 1 == 1 {
                    let present = s.present;
                    s.put(STATESTS, 2, u64::from(present));
                }
            }
            STATESTS | RIRBSTS | CORBSTS => {
                let old = s.get(at, n);
                s.put(at, n, old & !value);
            }
            CORBRP => {
                let keep = s.sticky_rp_reset && value & u64::from(CORBRP_RST) != 0;
                s.put(CORBRP, 2, if keep { u64::from(CORBRP_RST) } else { 0 });
            }
            RIRBWP => {
                if value & u64::from(RIRBWP_RST) != 0 {
                    s.put(RIRBWP, 2, 0);
                }
            }
            CORBSIZE | RIRBSIZE => {
                let cap = u64::from(s.corb_size & 0xF0);
                s.put(at, 1, cap | (value & 3));
            }
            CORBWP => {
                s.put(CORBWP, 2, value);
                s.run_corb();
            }
            at if at >= SD_BASE && (at - SD_BASE) % SD_STRIDE == SD_STS => {
                let old = s.get(at, 1);
                s.put(at, 1, old & !value);
            }
            at if at >= SD_BASE && (at - SD_BASE) % SD_STRIDE == SD_CTL0 => {
                s.put(at, 1, value);
                if value as u8 & SD_RUN != 0 {
                    s.run_stream(at);
                }
            }
            _ => s.put(at, n, value),
        }
    }
}

impl Regs for FakeRegs {
    fn read8(&self, offset: u64) -> u8 {
        let s = self.state();
        match offset {
            // The size registers' capability nibble is the controller's.
            CORBSIZE | RIRBSIZE => (s.corb_size & 0xF0) | (s.get(offset, 1) as u8 & 3),
            _ => s.get(offset, 1) as u8,
        }
    }
    fn read16(&self, offset: u64) -> u16 {
        self.state().get(offset, 2) as u16
    }
    fn read32(&self, offset: u64) -> u32 {
        self.state().get(offset, 4) as u32
    }
    fn write8(&self, offset: u64, value: u8) {
        self.write(offset, 1, value.into());
    }
    fn write16(&self, offset: u64, value: u16) {
        self.write(offset, 2, value.into());
    }
    fn write32(&self, offset: u64, value: u32) {
        self.write(offset, 4, value.into());
    }
}

impl Regs for &FakeRegs {
    fn read8(&self, offset: u64) -> u8 {
        (*self).read8(offset)
    }
    fn read16(&self, offset: u64) -> u16 {
        (*self).read16(offset)
    }
    fn read32(&self, offset: u64) -> u32 {
        (*self).read32(offset)
    }
    fn write8(&self, offset: u64, value: u8) {
        (*self).write8(offset, value)
    }
    fn write16(&self, offset: u64, value: u16) {
        (*self).write16(offset, value)
    }
    fn write32(&self, offset: u64, value: u32) {
        (*self).write32(offset, value)
    }
}

/// Widget capability words: the type in bits 23:20 and the flags below.
const STEREO: u32 = 1;
const IN_AMP: u32 = 1 << 1;
const OUT_AMP: u32 = 1 << 2;
const AMP_OVERRIDE: u32 = 1 << 3;
const FORMAT_OVERRIDE: u32 = 1 << 4;
const CONN_LIST: u32 = 1 << 8;
const DIGITAL: u32 = 1 << 9;
const POWER: u32 = 1 << 10;
const DAC: u32 = 0x0 << 20;
const ADC: u32 = 0x1 << 20;
const MIXER: u32 = 0x2 << 20;
const SELECTOR: u32 = 0x3 << 20;
const PIN: u32 = 0x4 << 20;
const PIN_OUT: u32 = 1 << 4;
const PIN_IN: u32 = 1 << 5;
const PIN_HP: u32 = 1 << 3;
const PIN_EAPD: u32 = 1 << 16;
/// Configuration defaults: a jack's line out, speaker, headphones, line in;
/// and a pin wired to nothing.
const LINE_OUT: u32 = 0x0101_0010;
const SPEAKER: u32 = 0x9017_0110;
const HEADPHONES: u32 = 0x0221_4020;
const LINE_IN: u32 = 0x0181_3030;
const NOTHING: u32 = 0x4000_00F0;

/// QEMU's `hda-duplex` codec, as its graph reads through the verbs: the root
/// (vendor 1af4, QEMU's), one audio function group at node 1 with nodes 2 to 5:
/// a DAC (2) feeding a line-out pin (3), and a line-in pin (5) feeding an ADC
/// (4). The node numbers and the shape are QEMU's; the capability words here
/// are this test's, written from the spec, and PR 2's boot logs the real ones.
fn qemu_duplex() -> FakeCodec {
    let mut c = FakeCodec::default();
    c.node(0).params.extend([
        (param::VENDOR_ID, 0x1AF4_0022),
        (param::NODE_COUNT, 0x0001_0001),
    ]);
    c.node(1).params.extend([
        (param::FUNCTION_GROUP_TYPE, 0x01),
        (param::NODE_COUNT, 0x0002_0004),
        (param::OUT_AMP_CAP, 0x8003_4A4A),
        (param::IN_AMP_CAP, 0x8003_4A4A),
    ]);
    c.widget(2, DAC | STEREO | OUT_AMP | FORMAT_OVERRIDE);
    let pin = c.widget(3, PIN | STEREO | CONN_LIST);
    pin.params.insert(param::PIN_CAP, PIN_OUT);
    pin.config = LINE_OUT;
    pin.conns = vec![2];
    c.widget(4, ADC | STEREO | IN_AMP | CONN_LIST).conns = vec![5];
    let input = c.widget(5, PIN | STEREO);
    input.params.insert(param::PIN_CAP, PIN_IN);
    input.config = LINE_IN;
    c
}

/// A PC codec's shape: two DACs, mixers and a selector between them and the
/// pins, a line out, a speaker, headphones with EAPD, a pin wired to nothing,
/// an input pin and a digital pin; widgets 0x02 to 0x1F.
fn pc_codec() -> FakeCodec {
    let mut c = FakeCodec::default();
    c.node(0).params.extend([
        (param::VENDOR_ID, 0x10EC_0897),
        (param::NODE_COUNT, 0x0001_0001),
    ]);
    c.node(1).params.extend([
        (param::FUNCTION_GROUP_TYPE, 0x01),
        (param::NODE_COUNT, 0x0002_001E),
        (param::OUT_AMP_CAP, 0x8005_3F3F),
        (param::IN_AMP_CAP, 0x8005_1F17),
    ]);
    c.widget(0x02, DAC | STEREO | OUT_AMP | POWER);
    c.widget(0x03, DAC | STEREO | OUT_AMP);
    c.widget(0x06, DAC | STEREO | DIGITAL);
    // Mixer 0x0C takes DAC 2 and input pin 0x18's path; 0x0D takes DAC 3.
    c.widget(0x0C, MIXER | STEREO | IN_AMP | CONN_LIST).conns = vec![0x02, 0x18];
    c.widget(0x0D, MIXER | STEREO | IN_AMP | AMP_OVERRIDE | CONN_LIST)
        .conns = vec![0x03];
    c.node(0x0D)
        .params
        .insert(param::IN_AMP_CAP, 0x8000_0000 | 0x11);
    // Selector 0x0F picks the input pin (0) or DAC 3 (1).
    c.widget(0x0F, SELECTOR | STEREO | OUT_AMP | CONN_LIST)
        .conns = vec![0x18, 0x03];
    let line = c.widget(0x14, PIN | STEREO | OUT_AMP | CONN_LIST);
    line.params.insert(param::PIN_CAP, PIN_OUT);
    line.config = LINE_OUT;
    line.conns = vec![0x0F];
    let speaker = c.widget(0x15, PIN | STEREO | OUT_AMP | CONN_LIST);
    speaker.params.insert(param::PIN_CAP, PIN_OUT | PIN_EAPD);
    speaker.config = SPEAKER;
    speaker.conns = vec![0x0D, 0x0C];
    let hp = c.widget(0x16, PIN | STEREO | OUT_AMP | CONN_LIST | POWER);
    hp.params
        .insert(param::PIN_CAP, PIN_OUT | PIN_HP | PIN_EAPD);
    hp.config = HEADPHONES;
    hp.conns = vec![0x0C, 0x0D];
    // Wired to nothing, though it could reach DAC 2 directly.
    let unwired = c.widget(0x17, PIN | STEREO | CONN_LIST);
    unwired.params.insert(param::PIN_CAP, PIN_OUT);
    unwired.config = NOTHING;
    unwired.conns = vec![0x02];
    let input = c.widget(0x18, PIN | STEREO);
    input.params.insert(param::PIN_CAP, PIN_IN);
    input.config = LINE_IN;
    // A digital (S/PDIF) pin to the digital converter is passed over.
    let spdif = c.widget(0x1E, PIN | DIGITAL | CONN_LIST);
    spdif.params.insert(param::PIN_CAP, PIN_OUT);
    spdif.config = LINE_OUT;
    spdif.conns = vec![0x06];
    c
}

impl Codec for FakeCodec {
    fn verb(&mut self, nid: u8, verb: Verb) -> Result<u32, Error> {
        Ok(self.answer(nid, verb.bits()))
    }
}

fn nids(path: &graph::Path) -> Vec<u8> {
    path.hops().iter().map(|h| h.nid).collect()
}

fn le16(bytes: &[u8], frame: usize) -> i16 {
    i16::from_le_bytes([bytes[frame * 4], bytes[frame * 4 + 1]])
}

#[test]
fn the_tone_plays_on_qemus_hda_duplex() {
    let regs = FakeRegs::new(qemu_duplex());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    let playing = hda.play_tone().unwrap();
    assert_eq!(playing.opened.version, (1, 0));
    assert_eq!(
        (
            playing.opened.codecs,
            playing.opened.codec,
            playing.opened.vendor
        ),
        (1, 0, 0x1AF4_0022)
    );
    assert_eq!(
        (
            playing.opened.group.nid,
            playing.opened.group.first,
            playing.opened.group.count
        ),
        (1, 2, 4)
    );
    assert_eq!(nids(&playing.opened.path), [3, 2], "pin 3 to DAC 2");
    assert_eq!(playing.opened.path.hops()[0].kind, Kind::Pin);
    // ICH6's four input streams come first: the first output stream is 4.
    assert_eq!((playing.tone.stream, playing.tone.buffer), (4, 8192));
    let s = regs.state();
    let c = &s.codec;
    assert!(c.sent(1, Verb::set_power_state(0)));
    assert!(c.sent(3, Verb::set_pin_control(0x40)));
    assert!(
        c.sent(2, Verb::set_amp(true, 0, false, 0x4A)),
        "DAC unmuted at 0 dB"
    );
    assert!(c.sent(2, Verb::set_converter_format(0x0011)));
    assert!(c.sent(2, Verb::set_stream_channel(1, 0)));
    assert!(!selected(c, 3), "one input, no select");
    let (fmt, tag, bytes) = s.played.clone().expect("the stream ran");
    assert_eq!((fmt, tag), (0x0011, 1));
    assert_eq!(bytes.len(), 8192);
    assert_eq!(le16(&bytes, 0), crate::stream::TONE_LEVEL);
    assert_eq!(le16(&bytes, 64), -crate::stream::TONE_LEVEL);
    assert!(bytes.iter().any(|&b| b != 0), "not silence");
    assert_eq!(s.get(sd(4, SD_CBL), 4), 8192);
    assert_eq!(s.get(sd(4, SD_LVI), 2), 1);
    drop(s);
    assert_eq!(hda.position(), Some(4096), "the position moved");
    hda.stop();
    assert_eq!(regs.read8(sd(4, SD_CTL0)) & SD_RUN, 0);
    assert_eq!(hda.position(), None);
}

/// Whether a set-connection-select (0x701) was sent to `nid`.
fn selected(c: &FakeCodec, nid: u8) -> bool {
    c.sets
        .iter()
        .any(|&(n, bits)| n == nid && bits >> 8 == 0x701)
}

#[test]
fn a_line_out_through_a_selector_is_taken_first() {
    let mut c = pc_codec();
    let group = graph::audio_function_group(&mut c).unwrap();
    assert_eq!((group.nid, group.first, group.count), (1, 2, 0x1E));
    let path = graph::output_path(&mut c, &group).unwrap();
    // Line out 0x14 -> selector 0x0F (input 1 of its list) -> DAC 3.
    assert_eq!(nids(&path), [0x14, 0x0F, 0x03]);
    assert_eq!(path.hops()[1].kind, Kind::Selector);
    assert_eq!((path.hops()[1].input, path.hops()[1].inputs), (1, 2));
    assert_eq!(path.device, crate::verb::Device::LineOut);

    // On the controller, the selector is set to input 1 and its output amp
    // unmuted at the function group's 0 dB (0x3F).
    let regs = FakeRegs::new(pc_codec());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    let playing = hda.play_tone().unwrap();
    assert_eq!(nids(&playing.opened.path), [0x14, 0x0F, 0x03]);
    let s = regs.state();
    assert!(s.codec.sent(0x0F, Verb::set_connection_select(1)));
    assert!(s.codec.sent(0x0F, Verb::set_amp(true, 0, false, 0x3F)));
    assert!(s.codec.sent(0x14, Verb::set_pin_control(0x40)));
    assert!(!selected(&s.codec, 0x14), "the pin has one input");
    assert!(s.codec.sent(0x03, Verb::set_stream_channel(1, 0)));
}

#[test]
fn without_a_line_out_the_speaker_then_the_headphones() {
    let mut c = pc_codec();
    c.node(0x14).config = NOTHING;
    let group = graph::audio_function_group(&mut c).unwrap();
    let path = graph::output_path(&mut c, &group).unwrap();
    // Speaker 0x15: its list is [0x0D, 0x0C]; both reach a DAC in two hops,
    // and the first found, 0x0D, wins.
    assert_eq!(nids(&path), [0x15, 0x0D, 0x03]);
    assert_eq!((path.hops()[0].input, path.hops()[0].inputs), (0, 2));

    let regs = FakeRegs::new(c.clone());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    hda.play_tone().unwrap();
    {
        let s = regs.state();
        let c = &s.codec;
        assert!(
            c.sent(0x15, Verb::set_connection_select(0)),
            "the pin picks 0x0D"
        );
        // Mixer 0x0D's own input amp caps: 0 dB at 0x11.
        assert!(c.sent(0x0D, Verb::set_amp(false, 0, false, 0x11)));
        assert!(c.sent(0x15, Verb::set_eapd(crate::verb::EAPD)));
        assert!(
            c.sent(0x15, Verb::set_pin_control(0x40)),
            "no HP amp on a speaker"
        );
    }

    c.node(0x15).config = NOTHING;
    let regs = FakeRegs::new(c);
    let mut hda = Controller::new(FakeHost::default(), &regs);
    let playing = hda.play_tone().unwrap();
    assert_eq!(nids(&playing.opened.path), [0x16, 0x0C, 0x02]);
    let s = regs.state();
    let c = &s.codec;
    assert!(c.sent(0x16, Verb::set_pin_control(0xC0)), "out and HP amp");
    assert!(c.sent(0x16, Verb::set_eapd(crate::verb::EAPD)));
    assert!(
        c.sent(0x16, Verb::set_power_state(0)),
        "it has power control"
    );
    assert!(c.sent(0x02, Verb::set_power_state(0)));
    // Mixer 0x0C's input 0 unmuted at the group's input 0 dB (0x17).
    assert!(c.sent(0x0C, Verb::set_amp(false, 0, false, 0x17)));
    assert!(!selected(c, 0x0C), "a mixer sums, it does not select");
}

#[test]
fn no_connected_analogue_output_is_no_path() {
    let mut c = pc_codec();
    for pin in [0x14, 0x15, 0x16] {
        c.node(pin).config = NOTHING;
    }
    let group = graph::audio_function_group(&mut c).unwrap();
    // Left: 0x17 wired to nothing, 0x18 input only, 0x1E digital.
    assert_eq!(graph::output_path(&mut c, &group), Err(Error::NoOutputPath));
    // A codec with only a modem group has no audio group.
    let mut modem = FakeCodec::default();
    modem.node(0).params.insert(param::NODE_COUNT, 0x0001_0001);
    modem
        .node(1)
        .params
        .insert(param::FUNCTION_GROUP_TYPE, 0x02);
    assert_eq!(
        graph::audio_function_group(&mut modem),
        Err(Error::NoAudioFunctionGroup)
    );
}

#[test]
fn a_cycle_or_a_long_chain_ends_the_walk() {
    // Pin 2 -> mixer 3 <-> mixer 4, and mixer 4 -> a chain of selectors
    // 5..=12 that ends at DAC 13, too far for MAX_PATH (8).
    let mut c = FakeCodec::default();
    c.node(0).params.insert(param::NODE_COUNT, 0x0001_0001);
    c.node(1).params.extend([
        (param::FUNCTION_GROUP_TYPE, 1),
        (param::NODE_COUNT, 0x0002_0010),
    ]);
    let pin = c.widget(2, PIN | CONN_LIST);
    pin.params.insert(param::PIN_CAP, PIN_OUT);
    pin.config = LINE_OUT;
    pin.conns = vec![3];
    c.widget(3, MIXER | CONN_LIST).conns = vec![4];
    c.widget(4, MIXER | CONN_LIST).conns = vec![3, 5];
    for n in 5..=12u8 {
        c.widget(n, SELECTOR | CONN_LIST).conns = vec![u16::from(n) + 1];
    }
    c.widget(13, DAC);
    let group = graph::audio_function_group(&mut c).unwrap();
    assert_eq!(graph::output_path(&mut c, &group), Err(Error::NoOutputPath));
    // Cut the chain at 8 and the DAC is in reach: exactly MAX_PATH hops.
    c.node(8).conns = vec![13];
    let path = graph::output_path(&mut c, &group).unwrap();
    assert_eq!(nids(&path), [2, 3, 4, 5, 6, 7, 8, 13]);
    assert_eq!(path.hops().len(), crate::graph::MAX_PATH);
}

#[test]
fn connection_lists_read_short_long_and_ranges() {
    let mut c = FakeCodec::default();
    // Short form, six entries over two responses: 2, 3, then 5 to 8 as a
    // range ending at 8 (0x88), then 0x20.
    c.node(9).conns = vec![2, 3, 5, 0x88, 0x20];
    let list = graph::connections(&mut c, 9).unwrap();
    assert_eq!(list.as_slice(), [2, 3, 5, 6, 7, 8, 0x20]);
    // Long form, two per response.
    let n = c.node(10);
    n.long = true;
    n.conns = vec![0x14, 0x15, 0x8018];
    let list = graph::connections(&mut c, 10).unwrap();
    assert_eq!(list.as_slice(), [0x14, 0x15, 0x16, 0x17, 0x18]);
    // A range first, a range backwards, a node past 255, a list past 32.
    c.node(11).conns = vec![0x85];
    assert!(matches!(
        graph::connections(&mut c, 11),
        Err(Error::Codec(_))
    ));
    c.node(11).conns = vec![9, 0x85];
    assert!(matches!(
        graph::connections(&mut c, 11),
        Err(Error::Codec(_))
    ));
    let n = c.node(12);
    n.long = true;
    n.conns = vec![0x0100];
    assert!(matches!(
        graph::connections(&mut c, 12),
        Err(Error::Codec(_))
    ));
    // 1, then 2 to 32: exactly 32.
    c.node(13).conns = vec![1, 0xA0];
    assert_eq!(graph::connections(&mut c, 13).unwrap().as_slice().len(), 32);
    c.node(13).conns = vec![1, 0xA1];
    assert!(matches!(
        graph::connections(&mut c, 13),
        Err(Error::Codec(_))
    ));
}

#[test]
fn a_codec_at_another_address_and_unsolicited_responses() {
    let regs = FakeRegs::new(qemu_duplex());
    {
        let mut s = regs.state();
        s.present = 0b100;
        s.address = 2;
        s.unsolicited = true;
        s.sticky_rp_reset = false;
    }
    let mut hda = Controller::new(FakeHost::default(), &regs);
    let playing = hda.play_tone().unwrap();
    assert_eq!((playing.opened.codecs, playing.opened.codec), (0b100, 2));
    assert_eq!(nids(&playing.opened.path), [3, 2]);
    // STATESTS was cleared after it was read.
    assert_eq!(regs.read16(STATESTS), 0);
}

#[test]
fn a_small_corb_wraps() {
    // A controller with 16-entry rings only: the walk sends far more than 16
    // commands, so both rings wrap many times.
    let regs = FakeRegs::new(pc_codec());
    regs.state().corb_size = 0x20;
    let mut hda = Controller::new(FakeHost::default(), &regs);
    let playing = hda.play_tone().unwrap();
    assert_eq!(nids(&playing.opened.path), [0x14, 0x0F, 0x03]);
    assert!(regs.state().commands > 3 * 16, "{}", regs.state().commands);
    assert_eq!(regs.read8(CORBSIZE) & 3, 1);
}

#[test]
fn what_stops_the_bring_up_is_named() {
    // No codec answers the reset.
    let regs = FakeRegs::new(qemu_duplex());
    regs.state().present = 0;
    assert_eq!(
        Controller::new(FakeHost::default(), &regs).play_tone(),
        Err(Error::NoCodec)
    );
    // The codec is there but never answers.
    let mut silent = qemu_duplex();
    silent.silent = true;
    let regs = FakeRegs::new(silent);
    assert_eq!(
        Controller::new(FakeHost::default(), &regs).play_tone(),
        Err(Error::NoResponse {
            nid: 0,
            verb: 0xF0000
        })
    );
    // No output stream.
    let regs = FakeRegs::new(qemu_duplex());
    regs.state().put(GCAP, 2, 0x0401);
    assert_eq!(
        Controller::new(FakeHost::default(), &regs).play_tone(),
        Err(Error::NoOutputStream)
    );
    // No ring size.
    let regs = FakeRegs::new(qemu_duplex());
    regs.state().corb_size = 0;
    assert_eq!(
        Controller::new(FakeHost::default(), &regs).play_tone(),
        Err(Error::Unsupported("no CORB size"))
    );
    // A command before the rings run.
    let regs = FakeRegs::new(qemu_duplex());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    assert!(matches!(
        hda.command(0, Verb::get_parameter(0)),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn a_32_bit_controller_gets_memory_below_4_gib() {
    // Without 64OK, memory comes from Below4G; this host's heap is above
    // 4 GiB, so the driver refuses it rather than truncating the address.
    let regs = FakeRegs::new(qemu_duplex());
    regs.state().put(GCAP, 2, 0x4400);
    let mut hda = Controller::new(FakeHost::default(), &regs);
    let got = hda.play_tone();
    assert_eq!(hda.host_mut().zones[0], Zone::Below4G);
    let heap_high = hda.host_mut().memory[0].as_ptr() as u64 >= 1 << 32;
    match heap_high {
        true => assert_eq!(got, Err(Error::Dma(DmaError::OutOfRange))),
        false => assert!(got.is_ok()),
    }
    let regs = FakeRegs::new(qemu_duplex());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    hda.play_tone().unwrap();
    assert_eq!(
        hda.host_mut().zones,
        [Zone::Any, Zone::Any],
        "rings, then the buffer"
    );
}

#[test]
fn ring_sizes_and_stream_counts() {
    assert_eq!(ring_size(0x42), Some((2, 256)));
    assert_eq!(ring_size(0x22), Some((1, 16)));
    assert_eq!(ring_size(0x10), Some((0, 2)));
    assert_eq!(ring_size(0x70), Some((2, 256)), "the largest offered");
    assert_eq!(ring_size(0x02), None);
    assert_eq!(streams(0x4401), (4, 4, 0));
    assert_eq!(streams(0x3218 | 1), (2, 3, 3));
    assert_eq!(sd(4, SD_FMT), 0x112);
    assert_eq!(sd(0, SD_CTL0), 0x80);
}

/// The kernel's register window is a `devkit::Mmio`: its 8- and 16-bit
/// accesses land at the register's own offset.
#[test]
fn mmio_is_regs() {
    let mut file = Box::new([0u32; 0x40]);
    let base = NonNull::new(file.as_mut_ptr()).unwrap();
    // SAFETY: `file` outlives `mmio` and holds 0x100 bytes.
    let mmio = unsafe { Mmio::new(base, 0x100) };
    Regs::write8(&mmio, CORBCTL, RING_RUN);
    Regs::write16(&mmio, CORBRP, 0x1234);
    Regs::write8(&mmio, CORBSIZE, 2);
    assert_eq!(Regs::read16(&mmio, CORBRP), 0x1234);
    assert_eq!(Regs::read32(&mmio, CORBCTL), 0x0002_0002);
    assert_eq!(file[CORBRP as usize / 4], 0x1234_0000);
}

#[test]
fn open_is_silent_and_the_tone_can_interrupt() {
    let regs = FakeRegs::new(qemu_duplex());
    let mut hda = Controller::new(FakeHost::default(), &regs);
    assert_eq!(hda.output_stream(), Ok(4));
    let opened = hda.open().unwrap();
    assert_eq!(nids(&opened.path), [3, 2]);
    assert!(regs.state().played.is_none(), "no stream runs after open");
    assert_eq!(hda.position(), None);
    assert!(regs.state().codec.sent(2, Verb::set_stream_channel(1, 0)));

    hda.set_interrupts(true);
    let tone = hda.start_tone(&opened).unwrap();
    assert_eq!((tone.stream, tone.buffer), (4, 8192));
    assert_eq!(regs.state().ioc, [true, true]);
    assert_eq!(regs.read32(INTCTL), INTCTL_GIE | 1 << 4);
    assert_eq!(regs.read8(sd(4, SD_CTL0)), SD_RUN | SD_IOCE);
    hda.stop();
    assert_eq!(regs.read32(INTCTL), 0);

    // Polled, the default: no IOC, no interrupt enables.
    let regs = FakeRegs::new(qemu_duplex());
    Controller::new(FakeHost::default(), &regs)
        .play_tone()
        .unwrap();
    assert_eq!(regs.state().ioc, [false, false]);
    assert_eq!(regs.read32(INTCTL), 0);
    assert_eq!(regs.read8(sd(4, SD_CTL0)), SD_RUN);
}
