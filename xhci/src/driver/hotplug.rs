//! Hot-plug over `fake`'s controller (P7 row 5): a device unplugged from a
//! root port is detached and its slot disabled, one plugged in is set up and
//! listened to, a slot and its pages are used again, and the devices already
//! running keep their reports meanwhile.

use super::fake::{Device, Fake};
use super::{Change, Controller, DeviceKind, Input, PortReport};
use crate::hal::Hal;
use crate::trb;
use std::vec;
use std::vec::Vec;

/// The boot keyboard's interrupt-IN endpoint, 0x81.
const KEYBOARD_DCI: u8 = 3;

fn start(fake: &Fake) -> Controller<Fake> {
    let mut controller = Controller::start(fake.clone()).expect("the fake controller starts");
    controller.enumerate(|report| assert!(report.outcome.is_ok(), "{report:?}"));
    controller
}

/// Polls, then services the ports, and returns the changes.
fn service(controller: &mut Controller<Fake>) -> Vec<Change> {
    controller.poll(|_| {});
    let mut changes = Vec::new();
    controller.service_ports(|change| changes.push(change));
    assert!(!controller.ports_changed(), "every change was serviced");
    changes
}

fn attached(change: &Change) -> PortReport {
    match change {
        Change::Attached(report) => *report,
        other => panic!("not an attach: {other:?}"),
    }
}

fn disables(fake: &Fake) -> usize {
    fake.commands()
        .iter()
        .filter(|&&kind| kind == trb::DISABLE_SLOT)
        .count()
}

#[test]
fn the_driver_s_own_port_resets_are_not_taken_for_a_new_device() {
    // Each port reset at boot writes a Port Status Change event, and the
    // ports start with their connect change set, as QEMU's do.
    let fake = Fake::new(vec![Some(Device::keyboard()), Some(Device::mouse())]);
    let mut controller = start(&fake);
    controller.poll(|_| {});
    assert!(
        controller.ports_changed(),
        "the resets' events mark the ports"
    );
    let mut changes = Vec::new();
    controller.service_ports(|change| changes.push(change));
    assert_eq!(changes, [], "nothing was plugged or unplugged");
    assert_eq!(disables(&fake), 0);
    assert_eq!((fake.slots(), controller.listening()), (2, 2));
}

#[test]
fn an_unplugged_keyboard_is_detached_and_its_slot_disabled() {
    let fake = Fake::new(vec![Some(Device::keyboard())]);
    let mut controller = start(&fake);
    assert_eq!(controller.listening(), 1);
    fake.unplug(1);
    assert_eq!(
        service(&mut controller),
        [Change::Detached { port: 1, route: 0 }]
    );
    assert_eq!(disables(&fake), 1);
    assert!(!fake.enabled(1));
    assert_eq!(controller.listening(), 0);
    // A second service with nothing new does nothing.
    assert_eq!(service(&mut controller), []);
    assert_eq!(disables(&fake), 1);
}

#[test]
fn a_keyboard_plugged_in_again_and_again_reuses_its_slot_and_pages() {
    let fake = Fake::new(vec![Some(Device::keyboard())]);
    let mut controller = start(&fake);
    let pages = fake.pages();
    // More times than there are device entries: a detach frees its entry.
    for round in 0..3 * super::MAX_DEVICES {
        fake.unplug(1);
        fake.plug(1, Device::keyboard());
        // Both changes are seen by one service: the old keyboard goes, the
        // new one is set up on the slot just disabled.
        let changes = service(&mut controller);
        assert_eq!(changes.len(), 2, "round {round}: {changes:?}");
        assert_eq!(changes[0], Change::Detached { port: 1, route: 0 });
        let report = attached(&changes[1]);
        let device = report.outcome.expect("the keyboard is set up");
        assert_eq!(device.kinds[0], Some(DeviceKind::Keyboard));
        assert_eq!((fake.slots(), fake.enabled(1)), (1, true));
        assert_eq!(controller.listening(), 1, "round {round}");
        assert_eq!(fake.pages(), pages, "round {round}: no page is taken anew");
    }
    // And it types.
    assert!(fake.report(1, KEYBOARD_DCI, &[0, 0, 4, 0, 0, 0, 0, 0]));
    let mut inputs = Vec::new();
    controller.poll(|input| inputs.push(input));
    assert_eq!(
        inputs,
        [Input::Key {
            usage: 4,
            pressed: true
        }]
    );
}

#[test]
fn an_unplugged_hub_takes_what_is_behind_it_along() {
    let hub = Device::hub(vec![Some(Device::keyboard()), None, Some(Device::mouse())]);
    let fake = Fake::new(vec![Some(hub)]);
    let mut controller = start(&fake);
    assert_eq!((fake.slots(), controller.listening()), (3, 2));
    fake.unplug(1);
    let mut changes = service(&mut controller);
    changes.sort_by_key(|change| match change {
        Change::Detached { route, .. } => *route,
        Change::Attached(_) => u32::MAX,
    });
    assert_eq!(
        changes,
        [0, 1, 3].map(|route| Change::Detached { port: 1, route })
    );
    assert_eq!(disables(&fake), 3);
    assert!((1..=3).all(|slot| !fake.enabled(slot)));
    assert_eq!(controller.listening(), 0);
}

#[test]
fn a_keyboard_keeps_typing_while_a_mouse_is_set_up() {
    let fake = Fake::new(vec![Some(Device::keyboard()), None]);
    let mut controller = start(&fake);
    controller.poll(|_| {});
    fake.plug(2, Device::mouse());
    controller.poll(|_| {});
    assert!(controller.ports_changed());
    // The key comes in while the mouse is being set up: the service's waits
    // read it off the event ring, and keep it.
    assert!(fake.report(1, KEYBOARD_DCI, &[0, 0, 4, 0, 0, 0, 0, 0]));
    let mut changes = Vec::new();
    controller.service_ports(|change| changes.push(change));
    let mouse = attached(&changes[0]).outcome.expect("the mouse is set up");
    assert_eq!(mouse.kinds[0], Some(DeviceKind::Mouse));
    let mut inputs = Vec::new();
    controller.poll(|input| inputs.push(input));
    assert_eq!(
        inputs,
        [Input::Key {
            usage: 4,
            pressed: true
        }],
        "the key was not lost"
    );
    // The keyboard is armed again and the mouse armed once.
    assert_eq!(controller.listening(), 2);
    assert!(fake.report(1, KEYBOARD_DCI, &[0; 8]));
    assert!(
        !fake.report(1, KEYBOARD_DCI, &[0; 8]),
        "one transfer at a time"
    );
}

#[test]
fn a_report_from_a_detached_slot_is_not_given_to_its_next_device() {
    let fake = Fake::new(vec![Some(Device::keyboard())]);
    let mut controller = start(&fake);
    controller.poll(|_| {});
    // A key, then the keyboard is unplugged and another plugged before the
    // poll: the key is read while the old slot is disabled.
    assert!(fake.report(1, KEYBOARD_DCI, &[0, 0, 4, 0, 0, 0, 0, 0]));
    fake.unplug(1);
    fake.plug(1, Device::keyboard());
    let mut changes = Vec::new();
    // Not polled first: the port events are read by the service's own waits.
    controller.mark(1);
    controller.service_ports(|change| changes.push(change));
    assert_eq!(changes[0], Change::Detached { port: 1, route: 0 });
    assert!(attached(&changes[1]).outcome.is_ok());
    let mut inputs = Vec::new();
    controller.poll(|input| inputs.push(input));
    assert_eq!(inputs, [], "the old keyboard's key is dropped with it");
    assert_eq!(controller.listening(), 1);
    assert!(fake.report(1, KEYBOARD_DCI, &[0; 8]));
    assert!(
        !fake.report(1, KEYBOARD_DCI, &[0; 8]),
        "one transfer at a time"
    );
}

/// Steps once, and returns its wait and the changes it reported.
fn step(controller: &mut Controller<Fake>) -> (Option<u64>, Vec<Change>) {
    let mut changes = Vec::new();
    let wait = controller.service_step(&mut |change| changes.push(change));
    (wait, changes)
}

#[test]
fn the_debounce_and_the_reset_are_waited_out_between_steps() {
    let fake = Fake::new(vec![Some(Device::keyboard()), None]);
    let mut controller = start(&fake);
    controller.poll(|_| {});
    controller.service_ports(|_| {});
    let slots = fake.slots();
    fake.plug(2, Device::mouse());
    controller.poll(|_| {});
    // The connect starts the debounce: nothing is set up within the step.
    let (wait, changes) = step(&mut controller);
    assert_eq!((wait, changes), (Some(super::enumerate::SETTLE_NS), vec![]));
    let (wait, _) = step(&mut controller);
    assert!(wait.is_some_and(|ns| ns > 0), "called early, it waits on");
    assert_eq!(fake.slots(), slots, "no slot before the debounce is over");
    // The keyboard's keys are read between the steps, as the interrupt does.
    assert!(fake.report(1, KEYBOARD_DCI, &[0, 0, 4, 0, 0, 0, 0, 0]));
    let mut inputs = Vec::new();
    controller.poll(|input| inputs.push(input));
    assert_eq!(inputs.len(), 1, "the key comes during the debounce");
    fake.delay(super::enumerate::SETTLE_NS);
    // Then the port is reset, and its recovery waited for, between steps.
    let (wait, _) = step(&mut controller);
    assert_eq!(wait, Some(super::ports::RESET_POLL_NS));
    let (wait, _) = step(&mut controller);
    assert_eq!(wait, Some(super::enumerate::RESET_RECOVERY_NS));
    assert_eq!(fake.slots(), slots, "nor before the reset's recovery");
    fake.delay(super::enumerate::RESET_RECOVERY_NS);
    let (wait, changes) = step(&mut controller);
    assert_eq!(wait, Some(0));
    let mouse = attached(&changes[0]).outcome.expect("the mouse is set up");
    assert_eq!(mouse.kinds[0], Some(DeviceKind::Mouse));
    assert_eq!(controller.listening(), 2);
    // The reset's own port change is not a connect: the service ends.
    controller.poll(|_| {});
    while let (Some(_), changes) = step(&mut controller) {
        assert_eq!(changes, [], "nothing more happens");
    }
}

#[test]
fn a_device_gone_before_its_debounce_is_over_is_not_set_up() {
    let fake = Fake::new(vec![None]);
    let mut controller = start(&fake);
    fake.plug(1, Device::keyboard());
    controller.poll(|_| {});
    assert!(step(&mut controller).0.is_some());
    fake.unplug(1);
    controller.poll(|_| {});
    fake.delay(super::enumerate::SETTLE_NS);
    let mut changes = Vec::new();
    while let (Some(_), mut more) = step(&mut controller) {
        changes.append(&mut more);
    }
    assert_eq!(changes, []);
    assert_eq!((fake.slots(), controller.listening()), (0, 0));
}
