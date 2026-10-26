//! D and the display: input, disks, the boot image and the screen.

use super::{KAI, TestCase, case};
use crate::disk::Fixture;

/// Moves the mouse right 38 and down 40 and clicks; then holds Ctrl for 1.5 s and
/// clicks again inside the hold. Mouse events skip the key queue that delays the
/// Ctrl release, so the second click lands while Ctrl is down.
const POINTER: &[&str] = &[
    "!mouse_move 38 40",
    "!mouse_button 1",
    "!mouse_button 0",
    "!sendkey ctrl 1500",
    "!mouse_button 1",
    "!mouse_button 0",
];

/// "kai" and Enter on the USB keyboard, then, through QMP, the USB tablet moves
/// to the middle of its X range and a quarter down, and the left button clicks.
const USB_TABLET: &[&str] = &[
    "k",
    "a",
    "i",
    "ret",
    r#"@{"execute":"input-send-event","arguments":{"events":[{"type":"abs","data":{"axis":"x","value":16384}},{"type":"abs","data":{"axis":"y","value":8192}}]}}"#,
    r#"@{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":true,"button":"left"}}]}}"#,
    r#"@{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":false,"button":"left"}}]}}"#,
];

/// "kai" and Enter on the USB keyboard, then the USB mouse moves right 38 and
/// down 40 and clicks.
const USB_MOUSE: &[&str] = &[
    "k",
    "a",
    "i",
    "ret",
    "!mouse_move 38 40",
    "!mouse_button 1",
    "!mouse_button 0",
];

pub(super) const CASES: &[TestCase] = &[
    TestCase {
        keys: &[KAI],
        ..case("keyboard", "test=keyboard")
    },
    TestCase {
        keys: &[POINTER],
        expect: &["pointer: at 38 40 cell 2 5", "pointer: click 1 ctrl"],
        ..case("pointer", "test=pointer pointerlog")
    },
    // ADR 0007: the PCI walk follows bridges, and devmatch binds what it finds.
    TestCase {
        bridge: true,
        dns: true,
        server: false,
        expect: &[
            "pci: virtio-net at 01:00.0 (id 1af4:",
            "net-static: 3 of 3 pings to the gateway answered",
        ],
        ..case("pci-bridge", "test=net-static ip=static")
    },
    // ADR 0007 section 8: reboot through the FADT's reset register (q35 has
    // port 0xCF9), and through the 8042 when told to skip it.
    TestCase {
        resets: true,
        expect: &["power: rebooting through the FADT reset register (write 0xf to I/O port 0xcf9)"],
        ..case("reboot", "test=reboot")
    },
    TestCase {
        resets: true,
        expect: &["power: rebooting through the 8042 reset line"],
        ..case("reboot-8042", "test=reboot reboot=8042")
    },
    // ADR 0008 "fault": a panic or a failed check resets the machine instead of
    // halting (`panic=reset` asks for that in test mode).
    TestCase {
        resets: true,
        expect: &[
            "fault: panic-reset: a panic, on purpose; resetting the machine",
            "power: rebooting through the FADT reset register",
        ],
        ..case("panic-reset", "test=panic-reset panic=reset")
    },
    TestCase {
        resets: true,
        expect: &[
            "fault: panic-reset: a failed check, on purpose; resetting the machine",
            "power: rebooting through the FADT reset register",
        ],
        ..case("fail-reset", "test=panic-reset what=fail panic=reset")
    },
    // ADR 0007 section 3: a record per bound device, read by DEV_LIST from ring 3.
    TestCase {
        usb: &["usb-kbd"],
        expect: &[
            "devices: DEV_LIST from ring 3 lists them too",
            "virtio-net in the kernel, running, polled, 0 interrupts",
            "class 0c0330 xhci in the kernel, running, MSI-X, ",
        ],
        ..case("devices", "test=devices")
    },
    // P7: a keyboard and a tablet on xHCI. QEMU sends keys to the USB keyboard
    // and absolute moves to the tablet; the kernel checks the keys came over USB,
    // through the controller's MSI-X interrupt.
    TestCase {
        usb: &["usb-kbd", "usb-tablet"],
        keys: &[USB_TABLET],
        expect: &[
            ": keyboard 0627:0001, ",
            ": tablet 0627:0001, ",
            "usb: USB tablet at 16384 8192 of 32767 32767, buttons 0",
            "usb: USB tablet at 16384 8192 of 32767 32767, buttons 1",
            "usb: received \"kai\" over USB",
            // ADR 0007: the controller interrupts on MSI-X; nothing polls it.
            "slots, interrupts on MSI-X vector 0x30",
            " MSI-X interrupts, 0 stray vectors",
        ],
        ..case("usb-tablet", "test=usb")
    },
    // P7: a keyboard and a relative mouse (boot protocol) on xHCI.
    TestCase {
        usb: &["usb-kbd", "usb-mouse"],
        keys: &[USB_MOUSE],
        expect: &[
            ": mouse 0627:0001, ",
            "usb: USB mouse moved 38 40, buttons 0",
            "usb: USB mouse moved 0 0, buttons 1",
            "usb: received \"kai\" over USB",
        ],
        ..case("usb-mouse", "test=usb")
    },
    TestCase {
        disk: Some(Fixture::Raw),
        expect: &[
            "blk: kai-model reads back byte for byte",
            "model: no GGUF magic; the kai-model disk is ignored",
        ],
        ..case("blk", "test=blk")
    },
    TestCase {
        disk: Some(Fixture::Gguf),
        expect: &[
            "model: llm (pid",
            "model: restarted llm (pid",
            "model: every process gone; USER_FRAMES 0, model frames still allocated",
        ],
        ..case("model", "test=model")
    },
    // The same disk checks over NVMe, the driver VirtualBox and real PCs need.
    TestCase {
        disk: Some(Fixture::Raw),
        nvme: true,
        expect: &[
            "blk: read 3146505 bytes of 6146 sectors over nvme",
            "blk: kai-model reads back byte for byte",
            "model: no GGUF magic; the kai-model disk is ignored",
        ],
        ..case("nvme", "test=blk")
    },
    TestCase {
        disk: Some(Fixture::Gguf),
        nvme: true,
        expect: &["nvme: namespace 1, ", "model: llm (pid"],
        ..case("nvme-model", "test=model")
    },
    // The boot disk VirtualBox gets: Limine and the kernel on a FAT32 ESP in a GPT
    // image, written by xtask, booted the way firmware boots a real disk.
    TestCase {
        image: true,
        expect: &["boot-ok"],
        ..case("image", "test")
    },
    // P3: the test pattern on virtio-gpu, in the mode the kernel picks, and the
    // hardware cursor set and moved; then in the mode `display=` asks for; then on
    // GOP, the fallback without the device.
    TestCase {
        gpu: true,
        keys: &[crate::screen::KEYS],
        expect: &[
            "display: virtio-gpu offers ",
            "display: cursor moved to 300 200",
        ],
        screen: Some(crate::screen::Expect {
            size: None,
            cursor: true,
        }),
        ..case("display", "test=display")
    },
    TestCase {
        gpu: true,
        keys: &[crate::screen::KEYS],
        expect: &["; 1024x768 (display=)", "display: virtio-gpu 1024x768"],
        screen: Some(crate::screen::Expect {
            size: Some((1024, 768)),
            cursor: true,
        }),
        ..case("display-mode", "test=display display=1024x768")
    },
    TestCase {
        keys: &[crate::screen::KEYS],
        expect: &["display: gop "],
        screen: Some(crate::screen::Expect {
            size: None,
            cursor: false,
        }),
        ..case("display-gop", "test=display")
    },
];
