//! Enumeration through hubs, over `fake`'s controller: the hub is configured and
//! its ports powered and reset with hub class requests, and each device behind it
//! gets a slot whose context carries its route string and, for a low- or
//! full-speed one, the transaction translator it talks through.

use super::fake::{Device, Fake};
use super::{Attached, Controller, DeviceKind, PortReport};
use crate::Error;
use crate::regs::speed;
use crate::trb;
use std::string::ToString;
use std::vec;
use std::vec::Vec;

/// Starts a controller over `fake` and enumerates it.
fn enumerate(fake: &Fake) -> (Controller<Fake>, Vec<PortReport>) {
    let mut controller = Controller::start(fake.clone()).expect("the fake controller starts");
    let mut reports = Vec::new();
    controller.enumerate(|report| reports.push(report));
    (controller, reports)
}

fn attached(report: &PortReport) -> Attached {
    report.outcome.expect("the device was set up")
}

fn routes(reports: &[PortReport]) -> Vec<(u8, u32)> {
    reports.iter().map(|r| (r.port, r.route)).collect()
}

#[test]
fn a_keyboard_and_a_mouse_behind_a_hub_are_set_up() {
    let fake = Fake::new(vec![Some(Device::hub(vec![
        Some(Device::keyboard()),
        None,
        Some(Device::mouse()),
        None,
    ]))]);
    let (controller, reports) = enumerate(&fake);
    // The devices behind the hub come first, then the hub.
    assert_eq!(routes(&reports), [(1, 1), (1, 3), (1, 0)]);
    assert_eq!(attached(&reports[0]).kinds[0], Some(DeviceKind::Keyboard));
    assert_eq!(attached(&reports[1]).kinds[0], Some(DeviceKind::Mouse));
    assert_eq!(attached(&reports[2]).hub_ports, Some(4));
    assert_eq!(controller.listening(), 2, "both endpoints are armed");
    assert_eq!(fake.slots(), 3);

    // The hub: slot 1, told it is a hub of four ports by Configure Endpoint.
    let hub = fake.slot(1);
    assert!(hub.hub && hub.addressed);
    assert_eq!(
        (hub.hub_ports, hub.speed, hub.root_port, hub.route),
        (4, speed::HIGH, 1, 0)
    );
    assert_eq!(hub.configures, 1);
    // The keyboard (full speed) is at route 1 and talks through the hub's
    // translator at port 1; the mouse (low speed) at route 3, port 3.
    let keyboard = fake.slot(2);
    assert_eq!(
        (keyboard.speed, keyboard.route, keyboard.root_port),
        (speed::FULL, 1, 1)
    );
    assert_eq!((keyboard.parent_slot, keyboard.parent_port), (1, 1));
    assert!(!keyboard.hub);
    let mouse = fake.slot(3);
    assert_eq!((mouse.speed, mouse.route), (speed::LOW, 3));
    assert_eq!((mouse.parent_slot, mouse.parent_port), (1, 3));
}

#[test]
fn a_device_behind_two_hubs_has_both_ports_in_its_route() {
    // Root port 2: a hub whose port 2 holds a hub whose port 1 holds a keyboard.
    let inner = Device::hub(vec![Some(Device::keyboard())]);
    let outer = Device::hub(vec![None, Some(inner)]);
    let fake = Fake::new(vec![None, Some(outer)]);
    let (_controller, reports) = enumerate(&fake);
    assert_eq!(routes(&reports), [(2, 0x12), (2, 0x02), (2, 0)]);
    // As the kernel logs them: the root port, then each hub's port in turn.
    let places: Vec<_> = reports.iter().map(|r| r.place().to_string()).collect();
    assert_eq!(places, ["2.2.1", "2.2", "2"]);
    // The keyboard's route names port 2 of the outer hub, then port 1 of the
    // inner; its translator is the inner hub's (the nearest high-speed hub).
    let keyboard = fake.slot(3);
    assert_eq!((keyboard.root_port, keyboard.route), (2, 0x12));
    assert_eq!((keyboard.parent_slot, keyboard.parent_port), (2, 1));
    // The inner hub's own context has route 2 and no translator: it is high speed.
    let inner = fake.slot(2);
    assert!(inner.hub);
    assert_eq!((inner.route, inner.parent_slot), (0x02, 0));
}

#[test]
fn a_low_speed_device_behind_a_full_speed_hub_keeps_the_translator_above_it() {
    // A high-speed hub, a full-speed hub on its port 2, a mouse on that hub.
    let slow = Device::slow_hub(vec![Some(Device::mouse())]);
    let fast = Device::hub(vec![None, Some(slow)]);
    let fake = Fake::new(vec![Some(fast)]);
    let (_controller, reports) = enumerate(&fake);
    assert!(reports.iter().all(|r| r.outcome.is_ok()), "{reports:?}");
    // The full-speed hub is behind the high-speed one's port 2: that is its
    // translator, and the mouse below it uses the same.
    let slow = fake.slot(2);
    assert_eq!((slow.parent_slot, slow.parent_port), (1, 2));
    let mouse = fake.slot(3);
    assert_eq!(mouse.route, 0x12);
    assert_eq!((mouse.parent_slot, mouse.parent_port), (1, 2));
}

#[test]
fn a_port_whose_reset_never_finishes_is_reported_and_the_others_go_on() {
    let mut stuck = Device::keyboard();
    stuck.stuck = true;
    let fake = Fake::new(vec![Some(Device::hub(vec![
        Some(stuck),
        Some(Device::mouse()),
    ]))]);
    let (_controller, reports) = enumerate(&fake);
    assert_eq!(reports[0].route, 1);
    assert_eq!(reports[0].outcome, Err(Error::Timeout));
    assert_eq!(attached(&reports[1]).kinds[0], Some(DeviceKind::Mouse));
    assert_eq!(attached(&reports[2]).hub_ports, Some(2));
}

#[test]
fn a_super_speed_hub_is_left_alone() {
    let fake = Fake::new(vec![Some(Device::super_hub(vec![
        Some(Device::keyboard()),
    ]))]);
    let (_controller, reports) = enumerate(&fake);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].outcome, Err(Error::Unsupported));
}

#[test]
fn hubs_nest_five_deep_and_no_further() {
    // Six hubs in a chain, each on port 1 of the last, a keyboard on the last.
    let mut chain = Device::hub(vec![Some(Device::keyboard())]);
    for _ in 0..5 {
        chain = Device::hub(vec![Some(chain)]);
    }
    let fake = Fake::new(vec![Some(chain)]);
    let (_controller, reports) = enumerate(&fake);
    // The sixth hub would have no nibble left in the route: it is reported, not
    // set up, and the keyboard behind it is not looked for.
    assert_eq!(reports[0].route, 0x11111);
    assert_eq!(reports[0].outcome, Err(Error::TooMany));
    assert_eq!(reports.len(), 6);
    assert!(
        reports[1..].iter().all(|r| r.outcome.is_ok()),
        "{reports:?}"
    );
}

#[test]
fn a_device_on_a_root_port_is_set_up_as_before() {
    let fake = Fake::new(vec![Some(Device::keyboard())]);
    let (controller, reports) = enumerate(&fake);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].route, 0);
    assert_eq!(attached(&reports[0]).hub_ports, None);
    assert_eq!(controller.listening(), 1);
    let slot = fake.slot(1);
    assert_eq!((slot.route, slot.parent_slot, slot.hub), (0, 0, false));
    // The slot context as Address Device read it: what `slot` always wrote.
    assert_eq!(slot.raw[..3], crate::context::slot(speed::FULL, 1, 1)[..3]);
    assert!(fake.commands().contains(&trb::ENABLE_SLOT));
}

/// Five hubs is the most the route string holds, and a device behind the fifth
/// is set up.
#[test]
fn a_device_behind_exactly_five_hubs_is_set_up() {
    let mut chain = Device::hub(vec![Some(Device::keyboard())]);
    for _ in 0..4 {
        chain = Device::hub(vec![Some(chain)]);
    }
    let fake = Fake::new(vec![Some(chain)]);
    let (controller, reports) = enumerate(&fake);
    assert!(reports.iter().all(|r| r.outcome.is_ok()), "{reports:?}");
    assert_eq!((reports[0].port, reports[0].route), (1, 0x11111));
    assert_eq!(attached(&reports[0]).kinds[0], Some(DeviceKind::Keyboard));
    assert_eq!(reports.len(), 6);
    assert_eq!(controller.listening(), 1);
    assert_eq!(fake.slot(6).route, 0x11111);
}

/// QEMU's usb-hub has 8 ports by default, and a descriptor of 10 bytes: the
/// device behind its last port is set up.
#[test]
fn a_hub_of_eight_ports_is_set_up_and_its_last_port_is_reached() {
    let mut ports = vec![None; 8];
    ports[7] = Some(Device::keyboard());
    ports[0] = Some(Device::mouse());
    let fake = Fake::new(vec![Some(Device::hub(ports))]);
    let (controller, reports) = enumerate(&fake);
    assert_eq!(routes(&reports), [(1, 1), (1, 8), (1, 0)]);
    assert_eq!(attached(&reports[0]).kinds[0], Some(DeviceKind::Mouse));
    assert_eq!(attached(&reports[1]).kinds[0], Some(DeviceKind::Keyboard));
    assert_eq!(attached(&reports[2]).hub_ports, Some(8));
    assert_eq!(controller.listening(), 2);
}

/// A hub whose every port hangs in its reset costs the boot a couple of
/// seconds, not a minute or two.
#[test]
fn a_broken_hub_holds_the_boot_for_a_bounded_time() {
    let ports = (0..15)
        .map(|_| {
            let mut stuck = Device::keyboard();
            stuck.stuck = true;
            Some(stuck)
        })
        .collect();
    let fake = Fake::new(vec![Some(Device::hub(ports))]);
    let (_controller, reports) = enumerate(&fake);
    assert_eq!(reports.len(), 16);
    assert!(
        reports[..15]
            .iter()
            .all(|r| r.outcome == Err(Error::Timeout)),
        "{reports:?}"
    );
    assert_eq!(attached(&reports[15]).hub_ports, Some(15));
    assert!(fake.now() < 3_000_000_000, "{} ns", fake.now());
}

/// A SuperSpeed port gives a hub that says USB 2 no pass.
#[test]
fn a_hub_on_a_super_speed_port_is_left_alone_whatever_its_descriptor_says() {
    let mut hub = Device::hub(vec![Some(Device::keyboard())]);
    hub.speed = speed::SUPER;
    let fake = Fake::new(vec![Some(hub)]);
    let (_controller, reports) = enumerate(&fake);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].outcome, Err(Error::Unsupported));
}
