//! Spike S06-3's devices, all port I/O: COM1's 8250, the 8254 PIT, the two
//! 8259 PICs, the CMOS clock and the probe port the guest's `s063probe` uses.
//! Every other port reads as all ones and ignores writes. Throwaway (ADR
//! 0006): the real `vmm` ports Firecracker's and crosvm's devices.

use crate::log;

/// The PIT's input clock, in Hz.
const PIT_HZ: u64 = 1_193_182;

/// Probe port: an OUT of a marker ([`Marker`]); an IN reads [`PROBE_MAGIC`].
pub const PROBE: u16 = 0x5E0;
/// Probe port: an OUT does nothing, the round trip `s063probe bench` times.
pub const PROBE_BENCH: u16 = 0x5E4;
/// Probe port: an IN reads the host's TSC rate in kHz.
pub const PROBE_TSC_KHZ: u16 = 0x5E8;
pub const PROBE_MAGIC: u32 = 0x5306_0003;

fn pit_ticks(ns: u64) -> u64 {
    (ns as u128 * PIT_HZ as u128 / 1_000_000_000) as u64
}

fn pit_ns(ticks: u64) -> u64 {
    (ticks as u128 * 1_000_000_000 / PIT_HZ as u128) as u64
}

/// One 8259, edge-triggered, fully nested, normal EOI.
#[derive(Default)]
pub struct Pic {
    imr: u8,
    irr: u8,
    isr: u8,
    base: u8,
    /// The next initialization word expected: 2, 3 or 4; 0 when initialized.
    init: u8,
    icw4: bool,
    single: bool,
    read_isr: bool,
}

impl Pic {
    fn write(&mut self, command: bool, v: u8) {
        if command {
            if v & 0x10 != 0 {
                *self = Self {
                    init: 2,
                    icw4: v & 1 != 0,
                    single: v & 2 != 0,
                    ..Self::default()
                };
            } else if v & 0x08 != 0 {
                if v & 2 != 0 {
                    self.read_isr = v & 1 != 0;
                }
            } else {
                match v >> 5 {
                    // Non-specific EOI: the highest-priority request in service.
                    0b001 => self.isr &= self.isr.wrapping_sub(1),
                    0b011 => self.isr &= !(1 << (v & 7)),
                    _ => {}
                }
            }
            return;
        }
        self.init = match self.init {
            2 => {
                self.base = v & 0xF8;
                match (self.single, self.icw4) {
                    (false, _) => 3,
                    (true, true) => 4,
                    (true, false) => 0,
                }
            }
            3 => match self.icw4 {
                true => 4,
                false => 0,
            },
            4 => 0,
            _ => {
                self.imr = v;
                0
            }
        };
    }

    fn read(&self, command: bool) -> u8 {
        match (command, self.read_isr) {
            (true, true) => self.isr,
            (true, false) => self.irr,
            (false, _) => self.imr,
        }
    }

    pub fn raise(&mut self, irq: u8) {
        self.irr |= 1 << irq;
    }

    /// The request to deliver next: unmasked, and above all in service.
    pub fn pending(&self) -> Option<u8> {
        let requests = self.irr & !self.imr;
        (0..8)
            .take_while(|i| self.isr & (1 << i) == 0)
            .find(|i| requests & (1 << i) != 0)
    }

    /// Takes `irq` into service; its vector.
    pub fn ack(&mut self, irq: u8) -> u8 {
        self.irr &= !(1 << irq);
        self.isr |= 1 << irq;
        self.base | irq
    }
}

#[derive(Clone, Copy, Default)]
struct Channel {
    mode: u8,
    access: u8,
    reload: u64,
    start: u64,
    latch: Option<u16>,
    read_high: bool,
    write_high: bool,
    low: u8,
}

impl Channel {
    fn elapsed(&self, now: u64) -> u64 {
        pit_ticks(now.saturating_sub(self.start))
    }

    fn count(&self, now: u64) -> u16 {
        let e = self.elapsed(now);
        match self.mode {
            2 | 3 => (self.reload - e % self.reload) as u16,
            _ => self.reload.wrapping_sub(e) as u16,
        }
    }

    fn out(&self, now: u64) -> bool {
        let e = self.elapsed(now);
        match self.mode {
            3 => e % self.reload < self.reload / 2,
            2 => true,
            _ => e >= self.reload,
        }
    }
}

/// The 8254, channel 0 on IRQ 0 and channel 2 behind port 0x61's gate.
pub struct Pit {
    ch: [Channel; 3],
    gate2: bool,
    speaker: bool,
    refresh: bool,
    /// When channel 0 next raises IRQ 0 (host nanoseconds).
    pub next_irq: Option<u64>,
}

impl Pit {
    fn new() -> Self {
        let ch = Channel {
            reload: 0x1_0000,
            ..Channel::default()
        };
        Self {
            ch: [ch; 3],
            gate2: false,
            speaker: false,
            refresh: false,
            next_irq: None,
        }
    }

    fn write(&mut self, port: u16, v: u8, now: u64) {
        if port == 0x43 {
            let c = (v >> 6) as usize;
            if c == 3 {
                return; // Read-back: not offered.
            }
            let ch = &mut self.ch[c];
            match (v >> 4) & 3 {
                0 => ch.latch = Some(ch.count(now)),
                access => {
                    ch.access = access;
                    ch.mode = match (v >> 1) & 7 {
                        6 => 2,
                        7 => 3,
                        m => m,
                    };
                    (ch.read_high, ch.write_high, ch.latch) = (false, false, None);
                    if c == 0 {
                        self.next_irq = None;
                    }
                }
            }
            return;
        }
        let c = (port - 0x40) as usize;
        let ch = &mut self.ch[c];
        let count = match ch.access {
            1 => v as u64,
            2 => (v as u64) << 8,
            _ if !ch.write_high => {
                (ch.low, ch.write_high) = (v, true);
                return;
            }
            _ => {
                ch.write_high = false;
                ch.low as u64 | (v as u64) << 8
            }
        };
        ch.reload = if count == 0 { 0x1_0000 } else { count };
        ch.start = now;
        if c == 0 {
            self.next_irq = Some(now + pit_ns(ch.reload));
        }
    }

    fn read(&mut self, port: u16, now: u64) -> u8 {
        let ch = &mut self.ch[(port - 0x40) as usize];
        let value = ch.latch.unwrap_or_else(|| ch.count(now));
        let high = match ch.access {
            2 => true,
            3 => {
                ch.read_high = !ch.read_high;
                !ch.read_high
            }
            _ => false,
        };
        if ch.latch.is_some() && (ch.access != 3 || high) {
            ch.latch = None;
        }
        if high { (value >> 8) as u8 } else { value as u8 }
    }

    fn port61(&mut self, now: u64) -> u8 {
        self.refresh = !self.refresh;
        self.gate2 as u8
            | (self.speaker as u8) << 1
            | (self.refresh as u8) << 4
            | (self.ch[2].out(now) as u8) << 5
    }

    fn set_port61(&mut self, v: u8, now: u64) {
        let gate = v & 1 != 0;
        if gate && !self.gate2 {
            self.ch[2].start = now;
        }
        (self.gate2, self.speaker) = (gate, v & 2 != 0);
    }

    /// Raises IRQ 0 for every expiry up to `now`; a run far behind (a host
    /// stall) drops the ticks it missed rather than storming the guest.
    fn update(&mut self, now: u64, pic: &mut Pic) {
        while let Some(at) = self.next_irq.filter(|&at| at <= now) {
            pic.raise(0);
            let ch = &self.ch[0];
            self.next_irq = match ch.mode {
                2 | 3 => {
                    let period = pit_ns(ch.reload);
                    let next = at + period;
                    Some(if next + 16 * period < now { now + period } else { next })
                }
                _ => None,
            };
        }
    }
}

/// COM1's 8250: no FIFO, nothing to receive, every byte sent at once.
pub struct Uart {
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    dll: u8,
    dlm: u8,
    /// A THR-empty interrupt is owed (cleared by reading IIR).
    thre: bool,
    line: [u8; 240],
    len: usize,
}

impl Uart {
    const fn new() -> Self {
        Self {
            ier: 0,
            lcr: 0,
            mcr: 0,
            scr: 0,
            dll: 0,
            dlm: 0,
            thre: false,
            line: [0; 240],
            len: 0,
        }
    }

    fn dlab(&self) -> bool {
        self.lcr & 0x80 != 0
    }

    pub fn wants_irq(&self) -> bool {
        self.thre && self.ier & 2 != 0
    }

    /// Returns the finished line, if `v` ended one.
    fn write(&mut self, off: u16, v: u8) -> Option<&str> {
        match off {
            0 if self.dlab() => self.dll = v,
            0 => {
                self.thre = true;
                if v == b'\n' || self.len == self.line.len() {
                    let line = core::str::from_utf8(&self.line[..self.len]).unwrap_or("?");
                    self.len = 0;
                    return Some(line);
                }
                if v != b'\r' {
                    self.line[self.len] = v;
                    self.len += 1;
                }
            }
            1 if self.dlab() => self.dlm = v,
            1 => {
                if v & 2 != 0 && self.ier & 2 == 0 {
                    self.thre = true;
                }
                self.ier = v & 0x0F;
            }
            3 => self.lcr = v,
            4 => self.mcr = v,
            7 => self.scr = v,
            _ => {}
        }
        None
    }

    fn read(&mut self, off: u16) -> u8 {
        match off {
            0 if self.dlab() => self.dll,
            1 if self.dlab() => self.dlm,
            1 => self.ier,
            2 if self.wants_irq() => {
                self.thre = false;
                0x02
            }
            2 => 0x01,
            3 => self.lcr,
            4 => self.mcr,
            5 => 0x60,
            6 => 0xB0,
            7 => self.scr,
            _ => 0,
        }
    }
}

/// What a probe OUT asks the host to record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    Init,
    DecodeStart,
    DecodeEnd,
    BenchStart,
    BenchEnd,
    Done,
    Other(u32),
}

impl Marker {
    fn of(v: u32) -> Self {
        match v {
            1 => Self::Init,
            2 => Self::DecodeStart,
            3 => Self::DecodeEnd,
            0x10 => Self::BenchStart,
            0x11 => Self::BenchEnd,
            0xFF => Self::Done,
            v => Self::Other(v),
        }
    }
}

/// What one port access did besides its value.
pub enum Effect {
    None,
    Line,
    Marker(Marker),
}

pub struct Devices {
    pub master: Pic,
    pub slave: Pic,
    pub pit: Pit,
    pub uart: Uart,
    cmos_index: u8,
    uart_irq: bool,
    pub tsc_khz: u32,
    /// The guest's kernel has logged that it runs /init.
    pub ran_init: bool,
    unknown: [u16; 8],
    pub bench_outs: u64,
}

/// The CMOS date: 2026-11-26 00:00:00, BCD.
const CMOS_DATE: [(u8, u8); 6] = [(0x07, 0x26), (0x08, 0x11), (0x09, 0x26), (0x32, 0x20), (0x06, 0x05), (0x00, 0)];

impl Devices {
    pub fn new(tsc_khz: u32) -> Self {
        Self {
            master: Pic::default(),
            slave: Pic::default(),
            pit: Pit::new(),
            uart: Uart::new(),
            cmos_index: 0,
            uart_irq: false,
            tsc_khz,
            ran_init: false,
            unknown: [0; 8],
            bench_outs: 0,
        }
    }

    /// Advances the PIT to `now`.
    pub fn update(&mut self, now: u64) {
        self.pit.update(now, &mut self.master);
    }

    /// An IN (`input`) or OUT of `size` bytes; an IN's value in the result.
    pub fn io(&mut self, port: u16, size: u8, input: bool, value: u32, now: u64) -> (u32, Effect) {
        let ones = match size {
            1 => 0xFF,
            2 => 0xFFFF,
            _ => 0xFFFF_FFFF,
        };
        let v8 = value as u8;
        let mut effect = Effect::None;
        let read = match port {
            0x3F8..=0x3FF if input => self.uart.read(port - 0x3F8) as u32,
            0x3F8..=0x3FF => {
                if let Some(line) = self.uart.write(port - 0x3F8, v8) {
                    log!("[guest] {line}");
                    self.ran_init |= line.contains("Run /init as init process");
                    effect = Effect::Line;
                }
                0
            }
            0x40..=0x42 if input => self.pit.read(port, now) as u32,
            0x40..=0x43 => {
                self.pit.write(port, v8, now);
                0
            }
            0x61 if input => self.pit.port61(now) as u32,
            0x61 => {
                self.pit.set_port61(v8, now);
                0
            }
            0x20 | 0x21 if input => self.master.read(port == 0x20) as u32,
            0x20 | 0x21 => {
                self.master.write(port == 0x20, v8);
                0
            }
            0xA0 | 0xA1 if input => self.slave.read(port == 0xA0) as u32,
            0xA0 | 0xA1 => {
                self.slave.write(port == 0xA0, v8);
                0
            }
            0x4D0 | 0x4D1 => 0,
            0x70 if !input => {
                self.cmos_index = v8 & 0x7F;
                0
            }
            0x71 if input => self.cmos(),
            0x80 | 0xED | 0x71 => 0,
            0x92 if input => 0x02,
            PROBE if input => PROBE_MAGIC,
            PROBE => {
                effect = Effect::Marker(Marker::of(value));
                0
            }
            PROBE_BENCH => {
                self.bench_outs += 1;
                0
            }
            PROBE_TSC_KHZ if input => self.tsc_khz,
            _ => {
                self.note_unknown(port);
                ones
            }
        };
        let irq = self.uart.wants_irq();
        if irq && !self.uart_irq {
            self.master.raise(4);
        }
        self.uart_irq = irq;
        (read & ones, effect)
    }

    fn cmos(&self) -> u32 {
        let v = match self.cmos_index {
            0x0A => 0x26,
            0x0B => 0x02,
            0x0D => 0x80,
            i => CMOS_DATE.iter().find(|(at, _)| *at == i).map_or(0, |(_, v)| *v),
        };
        v as u32
    }

    fn note_unknown(&mut self, port: u16) {
        if self.unknown.contains(&port) {
            return;
        }
        if let Some(slot) = self.unknown.iter_mut().find(|p| **p == 0) {
            *slot = port;
            log!("s06-3: port {port:#x} has no device; reads all ones");
        }
    }

    /// The vector to deliver next, taken into service; None when nothing is
    /// owed. The slave PIC is never asked for (no device uses it).
    pub fn take_interrupt(&mut self) -> Option<u8> {
        let irq = self.master.pending()?;
        Some(self.master.ack(irq))
    }

    pub fn has_interrupt(&self) -> bool {
        self.master.pending().is_some()
    }
}
