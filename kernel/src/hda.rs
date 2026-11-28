//! HD Audio (ADR 0007's audio row, v0.3) through the host-tested `hda` crate,
//! on the kernel's `devkit` host until `audiod` takes it (after S07-1). Each
//! controller is reset at boot and its first codec's output path opened, which
//! is silent; the tone plays only under `test=hda` or `hda=tone`, since on a
//! real PC it would sound from the speakers at every boot. A controller with
//! MSI interrupts at each buffer's end (ADR 0007 section 4: the handler only
//! acknowledges and counts); one without is polled.

use crate::devhost::KernelHost;
use crate::{hpet, log};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use devkit::{Host, Mmio};
use hda::regs::{SD_STS, SD_STS_CLEAR, sd};
use hda::verb::{Device, Kind};
use hda::{Controller, Opened, Tone};
use spin::{Mutex, Once};

/// An HD Audio controller the PCI probe found, BAR0 assigned by firmware.
pub struct Function {
    /// Where it sits on the bus, for log lines.
    pub name: String,
    pub bar: u64,
    pub bar_len: u64,
    /// Its MSI vector, delivered to the boot CPU; `None` to be polled.
    pub vector: Option<u8>,
}

/// BAR0 bytes mapped at most: the global registers, the rings' and every
/// stream descriptor of a controller with up to 30 streams fit in 4 KiB;
/// ICH6's BAR is 16 KiB.
const BAR_MAP_BYTES: u64 = 0x4000;

type KernelController = Controller<KernelHost, Mmio>;

/// The controller playing the tone, and its buffer's length.
static PLAYING: Mutex<Option<(KernelController, Tone)>> = Mutex::new(None);
/// Where the playing stream's status register is, for the interrupt handler
/// to acknowledge a buffer completion: set once, before the stream starts,
/// and read by the handler without a lock.
static STATUS: Once<Status> = Once::new();

/// The register window and the status register's offset in it.
struct Status(Mmio, u64);

// SAFETY: the handler only writes one status byte through `&self` (a
// volatile write to device memory); the window is mapped for good, and
// nothing else writes that register while the stream runs.
unsafe impl Sync for Status {}
/// MSI interrupts taken, for the self-test.
static INTERRUPTS: AtomicU64 = AtomicU64::new(0);

/// Opens each controller's output path and, under `test=hda` or `hda=tone`,
/// plays the tone on the first that opens. A controller that fails is logged,
/// and the kernel runs on without it.
pub fn init(functions: Vec<Function>, cmdline: &str) {
    let tone = cmdline
        .split_whitespace()
        .any(|w| w == "test=hda" || w == "hda=tone");
    for function in functions {
        let wanted = tone && PLAYING.lock().is_none();
        match start(&function, wanted) {
            Ok(Some(playing)) => *PLAYING.lock() = Some(playing),
            Ok(None) => {}
            Err(why) => log!("hda: {}: {why:?}", function.name),
        }
    }
}

fn start(function: &Function, tone: bool) -> Result<Option<(KernelController, Tone)>, hda::Error> {
    let mut host = KernelHost::new((function.bar, function.bar_len), function.vector);
    let regs = host
        .mmio(0, 0, function.bar_len.min(BAR_MAP_BYTES))
        .map_err(hda::Error::Dma)?;
    let mut controller = Controller::new(host, regs);
    let opened = controller.open()?;
    describe(&function.name, function.vector, &opened);
    if !tone {
        log!(
            "hda: {}: the output is open and silent (test=hda or hda=tone plays a tone)",
            function.name
        );
        // Stopped, no DMA left pointed at memory; the controller is dropped.
        if let Err(why) = controller.stop() {
            log!("hda: {}: stopping: {why:?}", function.name);
        }
        return Ok(None);
    }
    controller.set_interrupts(function.vector.is_some());
    if function.vector.is_some() {
        let stream = match controller.output_stream() {
            Ok(stream) => stream,
            Err(why) => {
                // Opened, so the rings run: stop them before giving up.
                let _ = controller.stop();
                return Err(why);
            }
        };
        STATUS.call_once(|| Status(regs, sd(stream, SD_STS)));
    }
    let tone = controller.start_tone(&opened)?;
    log!(
        "hda: {}: a 375 Hz tone queued on output stream {}: {} Hz, {}-bit, {} channels, a {}-byte cyclic buffer",
        function.name,
        tone.stream,
        tone.format.rate(),
        tone.format.bits,
        tone.format.channels,
        tone.buffer
    );
    Ok(Some((controller, tone)))
}

/// The controller, the codec and the output path, one line each.
fn describe(name: &str, vector: Option<u8>, opened: &Opened) {
    let interrupts = match vector {
        Some(vector) => format!("interrupts on MSI vector {vector:#x}"),
        None => String::from("polled"),
    };
    let (major, minor) = opened.version;
    log!(
        "hda: {name}: HD Audio {major}.{minor}, codecs {:#06x}, {interrupts}",
        opened.codecs
    );
    let group = opened.group;
    log!(
        "hda: {name}: codec {}, vendor {:08x}, audio function group at node {} (nodes {} to {})",
        opened.codec,
        opened.vendor,
        group.nid,
        group.first,
        u16::from(group.first) + u16::from(group.count).saturating_sub(1)
    );
    let mut path = String::new();
    for (i, hop) in opened.path.hops().iter().enumerate() {
        if i > 0 {
            path.push_str(" -> ");
        }
        let kind = match hop.kind {
            Kind::Pin => "pin",
            Kind::Mixer => "mixer",
            Kind::Selector => "selector",
            Kind::Output => "DAC",
            _ => "widget",
        };
        path.push_str(&format!("{kind} {}", hop.nid));
        if i == 0 {
            let device = match opened.path.device {
                Device::LineOut => "line out",
                Device::Speaker => "speaker",
                Device::HeadphoneOut => "headphones",
                Device::Other(_) => "other",
            };
            path.push_str(&format!(" ({device})"));
        }
    }
    log!("hda: {name}: output path {path}");
}

/// The playing stream's buffer completion: acknowledged and counted. Driver
/// work never runs here (ADR 0007 section 4).
pub fn on_interrupt() -> Option<usize> {
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
    if let Some(Status(regs, at)) = STATUS.get() {
        // Write 1 to clear: the completion, and any FIFO or descriptor error.
        regs.write8(*at, SD_STS_CLEAR);
    }
    None
}

/// v0.3 self-test: the tone's stream runs, so its position in the buffer
/// moves; with MSI, its buffer completions interrupt too.
pub fn self_test() -> Result<(), &'static str> {
    const TIMEOUT_NS: u64 = 30_000_000_000;
    let start = hpet::nanos();
    let mut seen: Vec<u32> = Vec::new();
    let msi = STATUS.get().is_some();
    let buffer = loop {
        let (position, buffer) = {
            let playing = PLAYING.lock();
            let (controller, tone) = playing.as_ref().ok_or("hda: no tone is playing")?;
            (
                controller.position().ok_or("hda: no stream is running")?,
                tone.buffer,
            )
        };
        if seen.last() != Some(&position) {
            seen.push(position);
        }
        let interrupted = !msi || INTERRUPTS.load(Ordering::Relaxed) >= 2;
        if seen.len() >= 3 && interrupted {
            break buffer;
        }
        if hpet::nanos().wrapping_sub(start) > TIMEOUT_NS {
            return Err(match seen.len() >= 3 {
                true => "hda: the stream moved but no buffer interrupt came within 30 s",
                false => "hda: the stream position did not move within 30 s",
            });
        }
        let pause = hpet::nanos();
        while hpet::nanos().wrapping_sub(pause) < 1_000_000 {
            core::hint::spin_loop();
        }
    };
    log!(
        "hda: the stream position moved: {}, {}, {} of {buffer} bytes",
        seen[0],
        seen[1],
        seen[2]
    );
    if msi {
        log!(
            "hda: {} buffer interrupts on MSI",
            INTERRUPTS.load(Ordering::Relaxed)
        );
    }
    Ok(())
}
