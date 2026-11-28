//! The controller: reset, the CORB and RIRB (HD Audio 1.0a sections 4.3 and
//! 4.4), commands to a codec, setting up an output path, and one output
//! stream playing the tone. It reaches memory and time through a
//! [`devkit::Host`] and the registers through [`Regs`], so the same code runs
//! on the kernel's host now and on `audiod`'s later.

use core::ptr::NonNull;

use devkit::{Dma, Host, Mmio, Zone};

use crate::Error;
use crate::graph::{self, Codec, FunctionGroup, Path};
use crate::regs::*;
use crate::stream::{self, Format};
use crate::verb::{self, Device, Kind, PIN_HP_ENABLE, PIN_OUT_ENABLE, Response, Verb, param};

/// The controller's registers at their own offsets and widths.
pub trait Regs {
    fn read8(&self, offset: u64) -> u8;
    fn read16(&self, offset: u64) -> u16;
    fn read32(&self, offset: u64) -> u32;
    fn write8(&self, offset: u64, value: u8);
    fn write16(&self, offset: u64, value: u16);
    fn write32(&self, offset: u64, value: u32);
}

impl Regs for Mmio {
    fn read8(&self, offset: u64) -> u8 {
        Mmio::read8(self, offset)
    }
    fn read16(&self, offset: u64) -> u16 {
        Mmio::read16(self, offset)
    }
    fn read32(&self, offset: u64) -> u32 {
        Mmio::read32(self, offset)
    }
    fn write8(&self, offset: u64, value: u8) {
        Mmio::write8(self, offset, value);
    }
    fn write16(&self, offset: u64, value: u16) {
        Mmio::write16(self, offset, value);
    }
    fn write32(&self, offset: u64, value: u32) {
        Mmio::write32(self, offset, value);
    }
}

/// How long a reset or a ring may take to answer.
const RESET_NS: u64 = 100_000_000;
/// How long a codec may take to answer one command.
const RESPONSE_NS: u64 = 10_000_000;
/// After CRST, codecs ask for an address within 521 µs (section 5.5.1).
const CODEC_WAKE_NS: u64 = 1_000_000;

/// The stream tag the output stream and its DAC share (1 to 15).
pub const STREAM_TAG: u8 = 1;
/// The tone's buffer: two pages, one BDL entry each, played round and round.
pub const BUFFER_PAGES: usize = 2;
const PAGE: usize = 4096;
/// Within the rings' page: the CORB (up to 256 entries of 4 bytes), the RIRB
/// (up to 256 of 8) and the BDL, each 128-byte aligned.
const CORB_AT: usize = 0;
const RIRB_AT: usize = 1024;
const BDL_AT: usize = 3072;

/// What the bring-up found and started, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Playing {
    pub version: (u8, u8),
    /// STATESTS: bit n is a codec at address n.
    pub codecs: u16,
    pub codec: u8,
    pub vendor: u32,
    pub group: FunctionGroup,
    pub path: Path,
    /// The stream descriptor the tone plays on.
    pub stream: u8,
    pub format: Format,
    /// The cyclic buffer's length in bytes.
    pub buffer: u32,
}

pub struct Controller<H: Host, R: Regs> {
    host: H,
    regs: R,
    rings: Option<Dma>,
    buffer: Option<Dma>,
    corb_entries: u16,
    rirb_entries: u16,
    corb_wp: u16,
    rirb_rp: u16,
    codec: u8,
    stream: Option<u8>,
}

impl<H: Host, R: Regs> Controller<H, R> {
    pub fn new(host: H, regs: R) -> Self {
        Controller {
            host,
            regs,
            rings: None,
            buffer: None,
            corb_entries: 0,
            rirb_entries: 0,
            corb_wp: 0,
            rirb_rp: 0,
            codec: 0,
            stream: None,
        }
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    /// Everything from reset to the tone playing: the controller out of reset,
    /// the rings running, the first codec's output path powered and unmuted,
    /// and the first output stream running on the tone.
    pub fn play_tone(&mut self) -> Result<Playing, Error> {
        let version = (self.regs.read8(VMAJ), self.regs.read8(VMIN));
        let codecs = self.reset()?;
        if codecs == 0 {
            return Err(Error::NoCodec);
        }
        self.codec = codecs.trailing_zeros() as u8;
        self.start_rings()?;
        let vendor = self.verb(0, Verb::get_parameter(param::VENDOR_ID))?;
        let group = graph::audio_function_group(self)?;
        let path = graph::output_path(self, &group)?;
        let format = Format::STEREO_48K_16;
        self.open_path(&group, &path, format)?;
        let (stream, buffer) = self.start_stream(format)?;
        Ok(Playing {
            version,
            codecs,
            codec: self.codec,
            vendor,
            group,
            path,
            stream,
            format,
            buffer,
        })
    }

    /// The output stream's position in its buffer, in bytes (SDnLPIB): it
    /// moves while the stream runs.
    pub fn position(&self) -> Option<u32> {
        self.stream.map(|s| self.regs.read32(sd(s, SD_LPIB)))
    }

    /// Stops the stream and the rings (the controller stays out of reset).
    pub fn stop(&mut self) {
        if let Some(s) = self.stream.take() {
            self.regs.write8(sd(s, SD_CTL0), 0);
        }
        self.regs.write8(CORBCTL, 0);
        self.regs.write8(RIRBCTL, 0);
    }

    fn wait(&self, what: &'static str, ns: u64, done: impl Fn(&R) -> bool) -> Result<(), Error> {
        let start = self.host.now_ns();
        loop {
            if done(&self.regs) {
                return Ok(());
            }
            if self.host.now_ns().wrapping_sub(start) > ns {
                return Err(Error::Timeout(what));
            }
            core::hint::spin_loop();
        }
    }

    /// Resets the link (section 4.2.2) and returns STATESTS, the codecs that
    /// woke.
    pub fn reset(&mut self) -> Result<u16, Error> {
        self.regs.write8(CORBCTL, 0);
        self.regs.write8(RIRBCTL, 0);
        self.wait("the rings to stop", RESET_NS, |r| {
            r.read8(CORBCTL) & RING_RUN == 0 && r.read8(RIRBCTL) & RING_RUN == 0
        })?;
        let gctl = self.regs.read32(GCTL);
        self.regs.write32(GCTL, gctl & !GCTL_CRST);
        self.wait("the link to enter reset", RESET_NS, |r| {
            r.read32(GCTL) & GCTL_CRST == 0
        })?;
        self.host.delay(100_000);
        self.regs.write32(GCTL, (gctl | GCTL_CRST) & !GCTL_UNSOL);
        self.wait("the link to leave reset", RESET_NS, |r| {
            r.read32(GCTL) & GCTL_CRST != 0
        })?;
        self.host.delay(CODEC_WAKE_NS);
        let codecs = self.regs.read16(STATESTS) & 0x7FFF;
        self.regs.write16(STATESTS, codecs);
        Ok(codecs)
    }

    fn zone(&self) -> Zone {
        match self.regs.read16(GCAP) & GCAP_64OK {
            0 => Zone::Below4G,
            _ => Zone::Any,
        }
    }

    fn dma(&mut self, pages: usize) -> Result<Dma, Error> {
        let zone = self.zone();
        let memory = self.host.dma(pages, zone).map_err(Error::Dma)?;
        let end = memory.device().checked_add(memory.len() as u64);
        if zone == Zone::Below4G && end.is_none_or(|e| e > 1 << 32) {
            return Err(Error::Dma(devkit::DmaError::OutOfRange));
        }
        Ok(memory)
    }

    /// Sets up and starts the CORB and RIRB in one page (sections 4.4.1.3 and
    /// 4.4.2.2).
    pub fn start_rings(&mut self) -> Result<(), Error> {
        let rings = match self.rings.take() {
            Some(rings) => rings,
            None => self.dma(1)?,
        };
        let base = rings.device();
        let (corb_size, corb_entries) =
            ring_size(self.regs.read8(CORBSIZE)).ok_or(Error::Unsupported("no CORB size"))?;
        let (rirb_size, rirb_entries) =
            ring_size(self.regs.read8(RIRBSIZE)).ok_or(Error::Unsupported("no RIRB size"))?;
        self.regs.write8(CORBSIZE, corb_size);
        self.regs.write32(CORBLBASE, (base + CORB_AT as u64) as u32);
        self.regs
            .write32(CORBUBASE, ((base + CORB_AT as u64) >> 32) as u32);
        // The read pointer's reset: some controllers show the bit set before
        // it is cleared, others clear it at once; only its clearing is waited.
        self.regs.write16(CORBRP, CORBRP_RST);
        let _ = self.wait("the CORB read pointer reset", 1_000_000, |r| {
            r.read16(CORBRP) & CORBRP_RST != 0
        });
        self.regs.write16(CORBRP, 0);
        self.wait("the CORB read pointer to leave reset", RESET_NS, |r| {
            r.read16(CORBRP) & CORBRP_RST == 0
        })?;
        self.regs.write16(CORBWP, 0);
        self.regs.write8(RIRBSIZE, rirb_size);
        self.regs.write32(RIRBLBASE, (base + RIRB_AT as u64) as u32);
        self.regs
            .write32(RIRBUBASE, ((base + RIRB_AT as u64) >> 32) as u32);
        self.regs.write16(RIRBWP, RIRBWP_RST);
        // One response per interrupt, so an interrupt-driven host later sees
        // each; polled, the status is cleared after every response.
        self.regs.write16(RINTCNT, 1);
        self.regs.write8(RIRBSTS, RIRBSTS_RINTFL | RIRBSTS_OIS);
        self.regs.write8(RIRBCTL, RING_RUN);
        self.regs.write8(CORBCTL, RING_RUN);
        self.wait("the rings to run", RESET_NS, |r| {
            r.read8(CORBCTL) & RING_RUN != 0 && r.read8(RIRBCTL) & RING_RUN != 0
        })?;
        self.corb_entries = corb_entries;
        self.rirb_entries = rirb_entries;
        self.corb_wp = 0;
        self.rirb_rp = 0;
        self.rings = Some(rings);
        Ok(())
    }

    fn rings_ptr(&self) -> Result<NonNull<u8>, Error> {
        self.rings
            .as_ref()
            .map(Dma::cpu)
            .ok_or(Error::Unsupported("the rings are not running"))
    }

    /// Sends one command to the codec and waits for its answer; unsolicited
    /// responses, and any from another codec, are passed over.
    pub fn command(&mut self, nid: u8, verb: Verb) -> Result<u32, Error> {
        let rings = self.rings_ptr()?;
        let entry = verb::command(self.codec, nid, verb);
        self.corb_wp = (self.corb_wp + 1) % self.corb_entries;
        let at = CORB_AT + 4 * usize::from(self.corb_wp);
        // SAFETY: the CORB lies in the rings' page, `at` + 4 inside it (256
        // entries of 4 bytes from 0), and the controller reads it only after
        // CORBWP moves.
        unsafe {
            rings
                .as_ptr()
                .add(at)
                .cast::<u32>()
                .write_volatile(entry.to_le())
        };
        self.regs.write16(CORBWP, self.corb_wp);
        let start = self.host.now_ns();
        loop {
            let wp = self.regs.read16(RIRBWP) & 0xFF;
            while self.rirb_rp != wp % self.rirb_entries {
                self.rirb_rp = (self.rirb_rp + 1) % self.rirb_entries;
                let at = RIRB_AT + 8 * usize::from(self.rirb_rp);
                // SAFETY: the RIRB lies in the rings' page (256 entries of 8
                // bytes from 1024), and the controller wrote this entry before
                // moving RIRBWP past it.
                let (low, high) = unsafe {
                    let p = rings.as_ptr().add(at).cast::<u32>();
                    (
                        u32::from_le(p.read_volatile()),
                        u32::from_le(p.add(1).read_volatile()),
                    )
                };
                self.regs.write8(RIRBSTS, RIRBSTS_RINTFL | RIRBSTS_OIS);
                let response = Response::from_entry(low, high);
                if !response.unsolicited && response.codec == self.codec {
                    return Ok(response.value);
                }
            }
            if self.host.now_ns().wrapping_sub(start) > RESPONSE_NS {
                return Err(Error::NoResponse {
                    nid,
                    verb: verb.bits(),
                });
            }
            core::hint::spin_loop();
        }
    }

    /// Powers the path, drives the pin, picks each selector's input, unmutes
    /// every amplifier on the way at 0 dB, and points the DAC at the stream.
    fn open_path(
        &mut self,
        group: &FunctionGroup,
        path: &Path,
        format: Format,
    ) -> Result<(), Error> {
        self.verb(group.nid, Verb::set_power_state(0))?;
        let out_amp = self.verb(group.nid, Verb::get_parameter(param::OUT_AMP_CAP))?;
        let in_amp = self.verb(group.nid, Verb::get_parameter(param::IN_AMP_CAP))?;
        for hop in path.hops() {
            if hop.caps.power_control() {
                self.verb(hop.nid, Verb::set_power_state(0))?;
            }
            if hop.inputs > 1 && matches!(hop.kind, Kind::Pin | Kind::Selector) {
                self.verb(hop.nid, Verb::set_connection_select(hop.input))?;
            }
            if hop.caps.out_amp() {
                let caps = match hop.caps.amp_override() {
                    true => self.verb(hop.nid, Verb::get_parameter(param::OUT_AMP_CAP))?,
                    false => out_amp,
                };
                let gain = verb::amp_offset(caps);
                self.verb(hop.nid, Verb::set_amp(true, 0, false, gain))?;
            }
            if hop.caps.in_amp() && matches!(hop.kind, Kind::Mixer | Kind::Selector) {
                let caps = match hop.caps.amp_override() {
                    true => self.verb(hop.nid, Verb::get_parameter(param::IN_AMP_CAP))?,
                    false => in_amp,
                };
                let gain = verb::amp_offset(caps);
                self.verb(hop.nid, Verb::set_amp(false, hop.input, false, gain))?;
            }
        }
        let headphones = path.device == Device::HeadphoneOut && path.pin_caps.headphone();
        let control = PIN_OUT_ENABLE | if headphones { PIN_HP_ENABLE } else { 0 };
        self.verb(path.pin(), Verb::set_pin_control(control))?;
        if path.pin_caps.eapd() {
            self.verb(path.pin(), Verb::set_eapd(verb::EAPD))?;
        }
        let bits = format
            .encode()
            .ok_or(Error::Unsupported("the stream format"))?;
        self.verb(path.dac(), Verb::set_converter_format(bits))?;
        self.verb(path.dac(), Verb::set_stream_channel(STREAM_TAG, 0))?;
        Ok(())
    }

    /// The first output stream descriptor, reset and started on the tone
    /// (section 4.5.1): returns its index and the buffer's length.
    fn start_stream(&mut self, format: Format) -> Result<(u8, u32), Error> {
        let (inputs, outputs, _) = streams(self.regs.read16(GCAP));
        if outputs == 0 {
            return Err(Error::NoOutputStream);
        }
        let s = inputs;
        self.regs.write8(sd(s, SD_CTL0), 0);
        self.regs.write8(sd(s, SD_CTL0), SD_SRST);
        let _ = self.wait("the stream to enter reset", 1_000_000, |r| {
            r.read8(sd(s, SD_CTL0)) & SD_SRST != 0
        });
        self.regs.write8(sd(s, SD_CTL0), 0);
        self.wait("the stream to leave reset", RESET_NS, |r| {
            r.read8(sd(s, SD_CTL0)) & SD_SRST == 0
        })?;
        self.regs.write8(sd(s, SD_STS), SD_STS_CLEAR);

        let buffer = match self.buffer.take() {
            Some(buffer) => buffer,
            None => self.dma(BUFFER_PAGES)?,
        };
        let rings = self.rings_ptr()?;
        let bdl = self.rings.as_ref().map(Dma::device).unwrap_or(0) + BDL_AT as u64;
        // SAFETY: the buffer is the host's `BUFFER_PAGES` pages, the driver's
        // alone, and the stream is in reset, so the device is not reading it.
        let bytes =
            unsafe { core::slice::from_raw_parts_mut(buffer.cpu().as_ptr(), BUFFER_PAGES * PAGE) };
        stream::tone(bytes);
        for page in 0..BUFFER_PAGES {
            let entry =
                stream::bdl_entry(buffer.device() + (page * PAGE) as u64, PAGE as u32, false);
            // SAFETY: the BDL is in the rings' page, from 3072, and two
            // 16-byte entries end well inside it.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    entry.as_ptr(),
                    rings.as_ptr().add(BDL_AT + page * stream::BDL_ENTRY),
                    stream::BDL_ENTRY,
                );
            }
        }
        let length = (BUFFER_PAGES * PAGE) as u32;
        self.regs.write32(sd(s, SD_BDPL), bdl as u32);
        self.regs.write32(sd(s, SD_BDPU), (bdl >> 32) as u32);
        self.regs.write32(sd(s, SD_CBL), length);
        self.regs.write16(sd(s, SD_LVI), (BUFFER_PAGES - 1) as u16);
        let bits = format
            .encode()
            .ok_or(Error::Unsupported("the stream format"))?;
        self.regs.write16(sd(s, SD_FMT), bits);
        self.regs.write8(sd(s, SD_CTL2), STREAM_TAG << 4);
        self.regs.write8(sd(s, SD_CTL0), SD_RUN);
        self.wait("the stream to run", RESET_NS, |r| {
            r.read8(sd(s, SD_CTL0)) & SD_RUN != 0
        })?;
        self.buffer = Some(buffer);
        self.stream = Some(s);
        Ok((s, length))
    }
}

impl<H: Host, R: Regs> Codec for Controller<H, R> {
    fn verb(&mut self, nid: u8, verb: Verb) -> Result<u32, Error> {
        self.command(nid, verb)
    }
}
