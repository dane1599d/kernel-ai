//! Interrupt endpoints' recovery (bug hunt 3, H3-5) over a fake HAL: a test
//! writes the controller's events into its event ring and reads back what the
//! driver queued on the command ring, the device's EP0 ring and the endpoint's.

use super::{Controller, Decode, Device, Endpoint, MAX_DEVICES, MAX_ENDPOINTS, Recovery};
use crate::hal::{Dma, Hal};
use crate::hid;
use crate::regs::Capabilities;
use crate::ring::{self, Consumer, Producer};
use crate::trb::{self, Trb};
use core::cell::RefCell;
use core::ptr::NonNull;
use std::boxed::Box;
use std::vec::Vec;

const SLOT: u8 = 1;
/// The keyboard's interrupt-IN endpoint, 0x81.
const DCI: u8 = 3;
/// The completion code of a USB transaction error.
const TRANSACTION_ERROR: u8 = 4;
/// Reset Endpoint's Transfer State Preserve bit.
const TSP: u32 = 1 << 9;

#[repr(C, align(4096))]
struct Page([u8; 4096]);

/// Pages from the heap at made-up device addresses; registers that read 0 and
/// keep what was written to them, the doorbells among it.
#[derive(Default)]
struct FakeHal {
    pages: u64,
    writes: RefCell<Vec<(usize, u32)>>,
}

impl Hal for FakeHal {
    fn dma(&mut self, pages: usize, _below_4g: bool) -> Option<Dma> {
        assert_eq!(pages, 1, "the driver asks for one page at a time");
        let page = Box::leak(Box::new(Page([0; 4096])));
        self.pages += 1;
        // SAFETY: a zeroed page, leaked so it outlives the controller, used by
        // nothing else; its device address is made up and only compared.
        Some(unsafe { Dma::new(self.pages << 20, NonNull::from(&mut page.0).cast(), 4096) })
    }

    fn read32(&self, _offset: usize) -> u32 {
        0
    }

    fn write32(&self, offset: usize, value: u32) {
        self.writes.borrow_mut().push((offset, value));
    }

    fn now_ns(&self) -> u64 {
        0
    }
}

fn page(hal: &mut FakeHal) -> Dma {
    hal.dma(1, false).expect("a page")
}

/// A boot keyboard on `SLOT` with its endpoint armed, as enumeration leaves it.
fn keyboard() -> Controller<FakeHal> {
    let mut hal = FakeHal::default();
    let dcbaa = page(&mut hal);
    let command_mem = page(&mut hal);
    let event_mem = page(&mut hal);
    let erst = page(&mut hal);
    let ep0_mem = page(&mut hal);
    let ring_mem = page(&mut hal);
    let device = Device {
        slot: SLOT,
        _output: page(&mut hal),
        input: page(&mut hal),
        ep0: Producer::new(ep0_mem.phys(), ring::ENTRIES),
        ep0_mem,
        data: page(&mut hal),
        reports: page(&mut hal),
    };
    let endpoint = Endpoint {
        slot: SLOT,
        dci: DCI,
        ring: Producer::new(ring_mem.phys(), ring::ENTRIES),
        ring_mem,
        device: 0,
        buffer: 0,
        len: 8,
        decode: Decode::BootKeyboard(hid::Keyboard::new()),
        armed: false,
        recovery: Recovery::Idle,
        failures: 0,
    };
    let mut controller = Controller {
        caps: Capabilities::parse(|_| 0),
        commands: Producer::new(command_mem.phys(), ring::ENTRIES),
        command_mem,
        events: Consumer::new(event_mem.phys(), ring::ENTRIES),
        event_mem,
        hal,
        dcbaa,
        erst,
        devices: [const { None }; MAX_DEVICES],
        endpoints: [const { None }; MAX_ENDPOINTS],
        errors: 0,
        hub_deadline: 0,
    };
    controller.devices[0] = Some(device);
    controller.endpoints[0] = Some(endpoint);
    controller.arm(0);
    controller
}

fn endpoint(c: &Controller<FakeHal>) -> &Endpoint {
    c.endpoints[0].as_ref().expect("the keyboard's endpoint")
}

fn device(c: &Controller<FakeHal>) -> &Device {
    c.devices[0].as_ref().expect("the keyboard")
}

/// TRB `index` of the ring in `mem`.
fn read(mem: &Dma, index: usize) -> Trb {
    let at = index * trb::BYTES;
    Trb([
        mem.read32(at),
        mem.read32(at + 4),
        mem.read32(at + 8),
        mem.read32(at + 12),
    ])
}

/// Writes event `n` (cycle 1: the ring's first pass) on `SLOT`, then polls.
fn event(c: &mut Controller<FakeHal>, n: usize, kind: u8, pointer: u64, code: u8, dci: u8) {
    let at = n * trb::BYTES;
    let status = u32::from(code) << 24;
    c.event_mem
        .write_dwords(at, &[pointer as u32, (pointer >> 32) as u32, status]);
    let control = 1 | (u32::from(kind) << 10) | (u32::from(dci) << 16) | (u32::from(SLOT) << 24);
    c.event_mem.write32(at + 12, control);
    c.poll(|_| {});
}

fn transfer(c: &mut Controller<FakeHal>, n: usize, pointer: u64, code: u8, dci: u8) {
    event(c, n, trb::TRANSFER_EVENT, pointer, code, dci);
}

/// Command `index` of the command ring completed.
fn completed(c: &mut Controller<FakeHal>, n: usize, index: u16) {
    let command = c.commands.address(index);
    event(c, n, trb::COMMAND_COMPLETION, command, trb::SUCCESS, 0);
}

#[test]
fn a_stall_is_cleared_on_the_device_before_the_endpoint_is_armed_again() {
    let mut c = keyboard();
    let first = endpoint(&c).ring.address(0);
    transfer(&mut c, 0, first, trb::STALL, DCI);
    let reset = read(&c.command_mem, 0);
    assert_eq!(reset.kind(), trb::RESET_ENDPOINT);
    assert_eq!(
        reset.0[3] & TSP,
        0,
        "a stall's reset starts the toggle over"
    );
    completed(&mut c, 1, 0);
    assert_eq!(read(&c.command_mem, 1).kind(), trb::SET_DEQUEUE);
    completed(&mut c, 2, 1);
    // CLEAR_FEATURE(ENDPOINT_HALT) for endpoint 0x81 on EP0, and no transfer
    // queued until it is done.
    let setup = read(&device(&c).ep0_mem, 0);
    assert_eq!(setup.kind(), trb::SETUP);
    assert_eq!([setup.0[0], setup.0[1]], [0x0000_0102, 0x0000_0081]);
    assert_eq!(read(&device(&c).ep0_mem, 1).kind(), trb::STATUS);
    assert_eq!(read(&endpoint(&c).ring_mem, 1), Trb::default());
    assert_eq!(c.listening(), 0);
    let status = device(&c).ep0.address(1);
    transfer(&mut c, 3, status, trb::SUCCESS, 1);
    assert_eq!(read(&endpoint(&c).ring_mem, 1).kind(), trb::NORMAL);
    assert_eq!(c.listening(), 1);
}

#[test]
fn a_refused_clear_leaves_the_endpoint_quiet() {
    let mut c = keyboard();
    let first = endpoint(&c).ring.address(0);
    transfer(&mut c, 0, first, trb::STALL, DCI);
    completed(&mut c, 1, 0);
    completed(&mut c, 2, 1);
    let setup = device(&c).ep0.address(0);
    transfer(&mut c, 3, setup, trb::STALL, 1);
    assert_eq!(read(&endpoint(&c).ring_mem, 1), Trb::default());
    assert_eq!((c.listening(), c.errors()), (0, 2));
    assert_eq!(endpoint(&c).recovery, Recovery::Idle);
}

#[test]
fn a_transaction_error_is_retried_with_its_toggle_kept() {
    let mut c = keyboard();
    let first = endpoint(&c).ring.address(0);
    transfer(&mut c, 0, first, TRANSACTION_ERROR, DCI);
    let reset = read(&c.command_mem, 0);
    assert_eq!(reset.kind(), trb::RESET_ENDPOINT);
    assert_eq!(reset.0[3] & TSP, TSP, "the toggle is kept");
    c.hal.writes.borrow_mut().clear();
    completed(&mut c, 1, 0);
    // The failed transfer runs again where it stopped: no Set TR Dequeue, no
    // second transfer, nothing on EP0, and the endpoint rung.
    assert_eq!(read(&c.command_mem, 1), Trb::default());
    assert_eq!(read(&endpoint(&c).ring_mem, 1), Trb::default());
    assert_eq!(read(&device(&c).ep0_mem, 0), Trb::default());
    assert_eq!(c.listening(), 1);
    let doorbell = (c.caps.doorbell(SLOT), u32::from(DCI));
    assert!(c.hal.writes.borrow().contains(&doorbell));
}
