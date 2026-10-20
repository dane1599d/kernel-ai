//! The probe's parsers and the Linux-side checklist over canned outputs, starting
//! from what the founder's Ubuntu printed on 2026-10-20. The doctor's tests use
//! the same outputs.

use super::check::{Needs, Place, WslConfig, linux, nested, parse_wslconfig};
use super::probe::{Kvm, NoDistro, choose, decode, mount_root, parse_list, parse_report};

/// The probe's output in the founder's Ubuntu: no KVM, no QEMU, no OVMF.
pub const FOUNDER_PROBE: &str = "kvm=missing\nvirt_flags=0\ncpus=16\nmem_kib=14132508\n\
    user=dane1\nqemu=\nwslg=yes\ndisplay=yes\nc_drive=/mnt/c/\nloopback=yes\ndone=yes\n";

/// The same distro with KVM working, after `apt-get install qemu-system-x86
/// qemu-system-gui ovmf`.
pub const READY_PROBE: &str = "kvm=usable\nvirt_flags=16\ncpus=16\nmem_kib=14132508\n\
    user=dane1\nqemu=/usr/bin/qemu-system-x86_64\n\
    qemu_version=QEMU emulator version 10.1.0 (Debian 1:10.1.0+ds-5ubuntu2)\n\
    displays=Available display backend types: none gtk sdl egl-headless curses dbus \n\
    ovmf=/usr/share/OVMF/OVMF_CODE_4M.fd|/usr/share/OVMF/OVMF_VARS_4M.fd\n\
    wslg=yes\ndisplay=yes\nc_drive=/mnt/c/\nloopback=yes\ndone=yes\n";

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[test]
fn the_probe_report_parses_and_a_cut_one_is_refused() {
    let report = parse_report(FOUNDER_PROBE).unwrap();
    assert_eq!(report.kvm, Kvm::Missing);
    assert_eq!(report.virt_flags, 0);
    assert_eq!((report.cpus, report.mem_kib), (Some(16), Some(14_132_508)));
    assert_eq!((report.qemu.as_deref(), report.ovmf.as_ref()), (None, None));
    assert_eq!(report.mount_root, "/mnt/");
    assert_eq!(report.loopback, Some(true));
    assert!(!report.window());
    let ready = parse_report(READY_PROBE).unwrap();
    assert!(ready.window());
    assert_eq!(
        ready.ovmf.as_ref().map(|(c, _)| c.as_str()),
        Some("/usr/share/OVMF/OVMF_CODE_4M.fd")
    );
    let cut = FOUNDER_PROBE.replace("done=yes\n", "");
    assert!(parse_report(&cut).is_err());
    let moved = READY_PROBE.replace("c_drive=/mnt/c/", "c_drive=/c/");
    assert_eq!(parse_report(&moved).unwrap().mount_root, "/");
    assert_eq!(mount_root("/mnt/c"), Some("/mnt/".into()));
    assert_eq!(mount_root(""), None);
}

#[test]
fn wsl_lists_decode_from_utf16_and_the_right_distro_is_chosen() {
    let listed = "  NAME              STATE           VERSION\r\n\
        * docker-desktop    Stopped         2\r\n\
          Ubuntu            En ejecución    2\r\n\
          Legacy            Running         1\r\n";
    let text = decode(&utf16(listed));
    assert_eq!(text, listed);
    assert_eq!(decode(listed.as_bytes()), listed);
    let distros = parse_list(&text);
    assert_eq!(distros.len(), 3);
    assert!(distros[0].default);
    // A state of two words, as another language may print, still parses.
    assert_eq!(
        (distros[1].name.as_str(), distros[1].version),
        ("Ubuntu", 2)
    );
    assert_eq!(
        (distros[2].name.as_str(), distros[2].version),
        ("Legacy", 1)
    );
    // Docker's distro is the default here, yet Ubuntu is used.
    assert_eq!(choose(&distros, None).unwrap().name, "Ubuntu");
    assert_eq!(choose(&distros, Some("legacy")).unwrap().name, "Legacy");
    let missing = choose(&distros, Some("Debian")).unwrap_err();
    assert!(
        matches!(&missing, NoDistro::NotListed(name, all) if name == "Debian" && all.len() == 3)
    );
    assert!(missing.describe().contains("KAI_WSL_DISTRO names Debian"));
    assert_eq!(choose(&[], None), Err(NoDistro::NoneListed));
}

#[test]
fn wslconfig_is_read_as_wsl_reads_it() {
    let none = parse_wslconfig("p", None);
    assert!(!none.exists && !none.has_wsl2 && none.nested.is_none());
    let text = "[wsl2] # the VM\n# nestedVirtualization=false\nNestedVirtualization = \"True\"\n\
        networkingMode=mirrored # shared localhost\n[experimental]\nnestedVirtualization=false\n";
    let config = parse_wslconfig("p", Some(text));
    assert!(config.has_wsl2);
    assert_eq!(config.nested, Some(true));
    assert_eq!(config.networking.as_deref(), Some("mirrored"));
    let founder = parse_wslconfig("p", Some("[wsl2]\nswapFile=D:\\\\WSL\\\\swap.vhdx\n"));
    assert!(founder.has_wsl2 && founder.nested.is_none());
    let off = parse_wslconfig("p", Some("[wsl2]\nnestedVirtualization=false\n"));
    assert_eq!(off.nested, Some(false));
}

fn wsl_place(config: &WslConfig) -> Place<'_> {
    Place {
        distro: Some("Ubuntu"),
        config,
    }
}

const RUN: Needs = Needs {
    window: true,
    loopback: false,
};
const TEST: Needs = Needs {
    window: false,
    loopback: true,
};
const ALL: Needs = Needs {
    window: true,
    loopback: true,
};

#[test]
fn a_boot_is_refused_with_the_fixes_for_what_it_lacks() {
    let config = parse_wslconfig("C:\\Users\\user\\.wslconfig", Some("[wsl2]\n"));
    let founder = parse_report(FOUNDER_PROBE).unwrap();
    let items = linux(&founder, wsl_place(&config), RUN);
    let names: Vec<&str> = items
        .iter()
        .filter(|i| !i.ok)
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "svm/vmx flag",
            "/dev/kvm in WSL",
            "QEMU in WSL",
            "QEMU window (WSLg)",
            "OVMF in WSL"
        ]
    );
    assert!(
        items[0].fix[0]
            .say
            .contains("nestedVirtualization on (above)")
    );
    let flag = &nested(&config, 0).fix;
    assert_eq!(
        flag[0].run.as_deref(),
        Some(r#"notepad "C:\Users\user\.wslconfig""#)
    );
    assert!(
        flag[0]
            .say
            .contains("under the line [wsl2], put the line nestedVirtualization=true")
    );
    assert_eq!(flag[1].run.as_deref(), Some("wsl --shutdown"));
    let qemu: Vec<_> = items[2]
        .fix
        .iter()
        .filter_map(|s| s.run.as_deref())
        .collect();
    assert_eq!(
        qemu,
        [
            "wsl -d Ubuntu -u root -- apt-get update",
            "wsl -d Ubuntu -u root -- apt-get install -y qemu-system-x86 qemu-system-gui ovmf",
        ]
    );
    // Every command stands alone on its line, with no words to paste along.
    for step in items.iter().flat_map(|i| &i.fix) {
        if let Some(command) = &step.run {
            assert!(
                !command.contains('(') && !command.contains(", then"),
                "{command}"
            );
        }
    }
    let ready = parse_report(READY_PROBE).unwrap();
    assert!(linux(&ready, wsl_place(&config), TEST).iter().all(|i| i.ok));
    let denied = parse_report(&READY_PROBE.replace("kvm=usable", "kvm=denied")).unwrap();
    let kvm = linux(&denied, wsl_place(&config), TEST).remove(1);
    assert_eq!(
        kvm.fix[0].run.as_deref(),
        Some("wsl -d Ubuntu -u root -- usermod -aG kvm dane1")
    );
    let no_gui = READY_PROBE
        .replace(" gtk sdl", "")
        .replace("display=yes", "display=no");
    let window = linux(&parse_report(&no_gui).unwrap(), wsl_place(&config), RUN).remove(3);
    assert_eq!(
        window.fix[1].run.as_deref(),
        Some("wsl -d Ubuntu -u root -- apt-get install -y qemu-system-gui")
    );
}

#[test]
fn nested_virtualization_and_loopback_fixes_follow_the_config() {
    let set = parse_wslconfig(
        "C:\\w",
        Some("[wsl2]\nnestedVirtualization=true\nnetworkingMode=Mirrored\n"),
    );
    assert!(nested(&set, 0).ok);
    let report = parse_report(&FOUNDER_PROBE.replace("loopback=yes", "loopback=no")).unwrap();
    let items = linux(&report, wsl_place(&set), ALL);
    // nestedVirtualization is already on: update WSL, else WHPX.
    let flag = &items[0].fix;
    assert_eq!(flag[0].run.as_deref(), Some("wsl --update"));
    assert!(flag[2].say.contains("WHPX"));
    // Already mirrored: the fix is not the same edit again.
    let loopback = items.last().unwrap();
    assert!(!loopback.ok && loopback.fix[0].say.contains("already mirrored"));
    let no_file = parse_wslconfig("C:\\w", None);
    let fix = &nested(&no_file, 0).fix[0];
    assert!(
        fix.say.contains("(new if missing), add a line [wsl2]"),
        "{fix:?}"
    );
    // On a Linux host the names lose "in WSL", and the fixes use sudo.
    let host = WslConfig::default();
    let place = Place {
        distro: None,
        config: &host,
    };
    let native = linux(&report, place, ALL);
    assert_eq!(native[1].name, "/dev/kvm");
    assert!(native[0].fix[0].say.contains("firmware (BIOS)"));
    assert_eq!(native[2].fix[0].run.as_deref(), Some("sudo apt-get update"));
}

/// CI's runner checks that the script itself runs under `sh` and ends.
#[cfg(target_os = "linux")]
#[test]
fn the_probe_script_runs_under_sh() {
    let report = super::probe::ask_native().unwrap();
    assert!(!report.user.is_empty() && report.cpus.unwrap_or(0) > 0);
    assert_eq!(report.loopback, None);
}
