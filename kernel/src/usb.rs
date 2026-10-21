//! USB keyboards, mice and tablets on xHCI controllers, through the host-tested
//! `xhci` crate. The PCI code has no MSI support yet, so the boot CPU polls
//! each controller's event ring from its timer tick. Keys become PS/2 set 1
//! scancodes in the i8042 ring, so the keyboard path cannot tell them apart;
//! pointer reports go to `pointer`'s event queue.

use crate::{hpet, i8042, keyboard, log, memory, percpu, pointer};
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use xhci::{Controller, DeviceKind, Dma, Hal, Input};

/// An xHCI controller the PCI probe found, with its BAR0 assigned by firmware.
pub struct Function {
    /// Where it sits on the bus, for log lines.
    pub name: String,
    pub bar: u64,
    pub bar_len: u64,
}

/// BAR0 bytes mapped at most: registers, runtime and doorbells of every
/// controller seen so far fit in 64 KiB.
const BAR_MAP_BYTES: u64 = 64 * 1024;

struct KernelHal {
    regs: u64,
    mapped: usize,
}

impl Hal for KernelHal {
    fn dma(&mut self, pages: usize, below_4g: bool) -> Option<Dma> {
        let phys = memory::with(|m| m.allocate_contiguous(pages as u64))?;
        let len = pages * xhci::PAGE;
        if below_4g && phys + len as u64 > 1 << 32 {
            // Left allocated: one page or so, on a controller that is rare.
            return None;
        }
        let virt = NonNull::new((memory::hhdm() + phys) as *mut u8)?;
        // SAFETY: the frames were just allocated for this driver alone and the direct
        // map covers them; zeroed here, they are never freed.
        unsafe {
            core::ptr::write_bytes(virt.as_ptr(), 0, len);
            Some(Dma::new(phys, virt, len))
        }
    }

    fn read32(&self, offset: usize) -> u32 {
        assert!(
            offset + 4 <= self.mapped,
            "usb: a register past the mapping"
        );
        // SAFETY: `regs` maps BAR0 uncached for `mapped` bytes, checked above.
        unsafe { ((self.regs + offset as u64) as *const u32).read_volatile() }
    }

    fn write32(&self, offset: usize, value: u32) {
        assert!(
            offset + 4 <= self.mapped,
            "usb: a register past the mapping"
        );
        // SAFETY: as in read32.
        unsafe { ((self.regs + offset as u64) as *mut u32).write_volatile(value) }
    }

    fn now_ns(&self) -> u64 {
        hpet::nanos()
    }
}

static CONTROLLERS: Mutex<Vec<Controller<KernelHal>>> = Mutex::new(Vec::new());
/// Set once the controllers are running, so the tick polls them.
static READY: AtomicBool = AtomicBool::new(false);
/// What was attached at boot, and what came since, for the self-test.
static KEYBOARDS: AtomicU64 = AtomicU64::new(0);
static POINTERS: AtomicU64 = AtomicU64::new(0);
static KEYS: AtomicU64 = AtomicU64::new(0);

/// Brings up each controller and the devices on its root ports. A controller
/// that fails is logged, and the kernel runs on without it.
pub fn init(functions: Vec<Function>) {
    let mut running = Vec::new();
    for function in functions {
        match start(&function) {
            Ok(controller) => running.push(controller),
            Err(message) => log!("usb: {} at {}", message, function.name),
        }
    }
    if running.is_empty() {
        return;
    }
    *CONTROLLERS.lock() = running;
    READY.store(true, Ordering::Release);
}

fn start(function: &Function) -> Result<Controller<KernelHal>, &'static str> {
    let mapped = function.bar_len.min(BAR_MAP_BYTES);
    let regs = memory::map_physical(function.bar, mapped, crate::virtio::hal::DEVICE)?;
    let hal = KernelHal {
        regs,
        mapped: mapped as usize,
    };
    let caps = xhci::regs::Capabilities::parse(|offset| hal.read32(offset));
    if caps.span() > hal.mapped {
        return Err("the registers lie past BAR0");
    }
    let mut controller = Controller::start(hal).map_err(xhci::Error::text)?;
    log!(
        "usb: xHCI {}.{:02x} at {}, {} ports, {} slots",
        caps.version >> 8,
        caps.version & 0xFF,
        function.name,
        caps.max_ports,
        caps.max_slots
    );
    controller.enumerate(|report| match report.outcome {
        Ok(device) => {
            for kind in device.kinds.iter().flatten() {
                let count = match kind {
                    DeviceKind::Keyboard => &KEYBOARDS,
                    DeviceKind::Mouse | DeviceKind::Tablet => &POINTERS,
                };
                count.fetch_add(1, Ordering::Relaxed);
                log!(
                    "usb: port {}: {} {:04x}:{:04x}, {}",
                    report.port,
                    kind.name(),
                    device.vendor,
                    device.product,
                    xhci::regs::speed::name(device.speed)
                );
            }
            if device.kinds.iter().all(Option::is_none) {
                log!(
                    "usb: port {}: {:04x}:{:04x} has no keyboard or pointer",
                    report.port,
                    device.vendor,
                    device.product
                );
            }
        }
        Err(error) => log!("usb: port {}: {}", report.port, error.text()),
    });
    Ok(controller)
}

/// Called from the timer interrupt: the boot CPU, which also takes IRQ1, polls
/// the controllers. Returns a thread to wake once the interrupt is acknowledged.
pub fn on_tick() -> Option<usize> {
    if !READY.load(Ordering::Acquire) || percpu::index() != 0 {
        return None;
    }
    let mut controllers = CONTROLLERS.try_lock()?;
    let mut bytes = [0u8; 64];
    let mut len = 0;
    for controller in controllers.iter_mut() {
        controller.poll(|input| match input {
            Input::Key { usage, pressed } => {
                if let Some(code) = xhci::hid::set1(usage, pressed)
                    && len + code.bytes().len() <= bytes.len()
                {
                    bytes[len..len + code.bytes().len()].copy_from_slice(code.bytes());
                    len += code.bytes().len();
                    KEYS.fetch_add(1, Ordering::Relaxed);
                }
            }
            Input::Pointer { report, .. } => pointer::push(pointer_event(report)),
        });
    }
    if len == 0 {
        return None;
    }
    i8042::inject(&bytes[..len])
}

fn pointer_event(report: xhci::hid::Pointer) -> pointer::Event {
    let (source, motion) = match report.range {
        Some((x_max, y_max)) => (
            pointer::Source::UsbTablet,
            pointer::Motion::Absolute {
                x: report.x.max(0) as u32,
                y: report.y.max(0) as u32,
                x_max,
                y_max,
            },
        ),
        None => (
            pointer::Source::UsbMouse,
            pointer::Motion::Relative {
                dx: report.x,
                dy: report.y,
            },
        ),
    };
    pointer::Event {
        source,
        motion,
        buttons: report.buttons,
    }
}

/// P7 self-test: the harness types "kai" and Enter on the USB keyboard, then
/// moves the USB pointer and clicks. Each pointer event is logged; the test
/// passes once the line came over USB and the left button went down.
pub fn self_test() -> Result<(), &'static str> {
    const TIMEOUT_NS: u64 = 60_000_000_000;
    if KEYBOARDS.load(Ordering::Relaxed) == 0 {
        return Err("usb: no USB keyboard was set up");
    }
    if POINTERS.load(Ordering::Relaxed) == 0 {
        return Err("usb: no USB mouse or tablet was set up");
    }
    log!("usb: ready for keys (\"kai\" and Enter, then a pointer move and a click)");
    let start = hpet::nanos();
    let mut decoder = keyboard::Decoder::new();
    let mut line = String::new();
    let (mut typed, mut clicked) = (false, false);
    while !(typed && clicked) {
        while let Some(key) = decoder.next_key() {
            match key {
                termcore::keys::Key::Enter if line == "kai" => typed = true,
                termcore::keys::Key::Enter => line.clear(),
                termcore::keys::Key::Char(ch) => line.push(ch),
                _ => {}
            }
        }
        while let Some(event) = pointer::next_event() {
            if event.source == pointer::Source::Ps2 {
                continue;
            }
            let motion = match event.motion {
                pointer::Motion::Relative { dx, dy } => alloc::format!("moved {dx} {dy}"),
                pointer::Motion::Absolute { x, y, x_max, y_max } => {
                    alloc::format!("at {x} {y} of {x_max} {y_max}")
                }
            };
            log!(
                "usb: {} {motion}, buttons {}",
                event.source.name(),
                event.buttons
            );
            clicked |= event.buttons & 1 != 0;
        }
        if hpet::nanos() - start > TIMEOUT_NS {
            return Err("usb: the line and a click did not both arrive within 60 s");
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
    let keys = KEYS.load(Ordering::Relaxed);
    // k, a, i and Enter, each down and up.
    if keys < 8 {
        return Err("usb: the line came from another keyboard, not over USB");
    }
    log!("usb: received \"kai\" over USB ({keys} key changes) and a left click");
    pointer::show();
    Ok(())
}
