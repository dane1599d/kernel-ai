//! The doctor's checks and advice over canned outputs, starting from what the
//! founder's PC showed on 2026-10-20, then each branch of the decision table.

use super::advice::{Advice, VmPath, advise};
use super::checks::{Facts, Feature, HostQemu, Space, WslFacts, items};
use super::facts::{base64, drive_letter, encode_command, keyvals, windows_query};
use crate::models::{Held, Inventory, Stocked};
use crate::wsl::check::{Item, WslConfig, parse_wslconfig};
use crate::wsl::check_tests::{FOUNDER_PROBE, READY_PROBE};
use crate::wsl::probe::{Distro, NoDistro, parse_report};
use std::path::PathBuf;

fn ubuntu(version: u32) -> Distro {
    Distro {
        name: "Ubuntu".into(),
        version,
        default: true,
    }
}

fn model(id: &str, name: &str, file: &str, size: u64, min_ram_gb: u64, held: Held) -> Stocked {
    Stocked {
        id: id.into(),
        name: name.into(),
        file: file.into(),
        size,
        held,
        min_ram_gb,
        runnable: true,
        vm_mib: crate::disk::memory_mib_for(size),
    }
}

/// The catalog since L6: the 4B is the default, from 8 GB of RAM. `held` is
/// the 4B's state; the others are not downloaded.
fn store(held: Held) -> Inventory {
    Inventory {
        dir: PathBuf::from(r"D:\kai-models"),
        default: "qwen3-4b-2507".into(),
        models: vec![
            model(
                "qwen3-0.6b",
                "Qwen3-0.6B Q8_0",
                "Qwen3-0.6B-Q8_0.gguf",
                639_446_688,
                4,
                Held::Missing,
            ),
            model(
                "qwen3-1.7b",
                "Qwen3-1.7B Q4_K_M",
                "Qwen3-1.7B-Q4_K_M.gguf",
                1_107_409_472,
                4,
                Held::Missing,
            ),
            model(
                "qwen3-4b-2507",
                "Qwen3-4B-Instruct-2507 Q4_K_M",
                "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
                2_497_281_120,
                8,
                held,
            ),
        ],
    }
}

/// The founder's PC as the doctor saw it.
fn founder() -> Facts {
    Facts {
        windows: true,
        avx2_fma: true,
        avx512: true,
        ram_bytes: Some(29_766_000_000),
        space: vec![
            Space {
                place: "C:".into(),
                free: Some(14 << 30),
            },
            Space {
                place: "D:".into(),
                free: Some(306 << 30),
            },
        ],
        store: Ok(store(Held::Missing)),
        store_env: None,
        qemu: None,
        whpx: Feature::Off,
        hypervisor: Some(true),
        wsl: Some(WslFacts {
            version: Some("WSL version: 2.7.8.0".into()),
            distro: Ok(ubuntu(2)),
            probe: parse_report(FOUNDER_PROBE),
            note: Some(
                "wsl: Failed to configure network (networkingMode Nat), falling back to \
                 networkingMode VirtioProxy."
                    .into(),
            ),
        }),
        wslconfig: parse_wslconfig(
            "C:\\Users\\user\\.wslconfig",
            Some("[wsl2]\nswapFile=D:\\\\WSL\\\\swap.vhdx\n"),
        ),
        native: None,
    }
}

fn with_wsl(mut facts: Facts, edit: impl FnOnce(&mut WslFacts)) -> Facts {
    edit(facts.wsl.as_mut().unwrap());
    facts
}

fn item<'a>(items: &'a [Item], name: &str) -> &'a Item {
    items
        .iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("no item {name} in {items:#?}"))
}

/// The commands of the advice in order, and the words of steps without one.
fn lines(advice: &Advice) -> Vec<String> {
    advice
        .steps
        .iter()
        .map(|s| s.run.clone().unwrap_or_else(|| s.say.clone()))
        .collect()
}

#[test]
fn powershell_answers_and_encodings() {
    let values = keyvals("ram=34266345472\r\nhypervisor=True\r\nwhpx=2\r\nfree_C=16106127360\r\n");
    assert_eq!(values["hypervisor"], "True");
    assert_eq!(Feature::from_install_state(&values["whpx"]), Feature::Off);
    assert_eq!(Feature::from_install_state("1"), Feature::On);
    assert_eq!(Feature::from_install_state(""), Feature::Unknown);
    let cases = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v")];
    for (plain, coded) in cases {
        assert_eq!(base64(plain.as_bytes()), coded);
    }
    assert_eq!(encode_command("a"), "YQA=");
    assert_eq!(drive_letter("\\\\?\\d:\\kai-models"), Some('D'));
    assert_eq!(drive_letter("/home/me"), None);
    // A missing drive must not make PowerShell fail the whole query.
    assert!(windows_query(&['C', 'Q']).ends_with("\nexit 0"));
}

#[test]
fn the_founders_pc_fails_on_nested_virtualization_and_packages() {
    let list = items(&founder());
    let ok = |name: &str| item(&list, name).ok;
    assert!(ok("CPU") && ok("RAM") && ok("Free on C:") && ok("Free on D:"));
    assert!(ok("Hypervisor (hypervisorlaunchtype)") && ok("WSL"));
    assert!(ok("WSL → Windows localhost"));
    for failing in [
        "Model store",
        "QEMU for Windows",
        "Windows Hypervisor Platform",
        "nestedVirtualization",
        "svm/vmx flag",
        "/dev/kvm in WSL",
        "QEMU in WSL",
        "QEMU window (WSLg)",
        "OVMF in WSL",
    ] {
        let failed = item(&list, failing);
        assert!(!failed.ok && !failed.fix.is_empty(), "{failed:?}");
    }
    let nested = item(&list, "nestedVirtualization");
    assert_eq!(
        nested.fix[0].run.as_deref(),
        Some(r#"notepad "C:\Users\user\.wslconfig""#)
    );
    assert_eq!(nested.fix[1].run.as_deref(), Some("wsl --shutdown"));
    let store = item(&list, "Model store");
    assert_eq!(
        store.fix[0].run.as_deref(),
        Some("cargo xtask setup qwen3-4b-2507")
    );
    assert!(item(&list, "WSL").found.contains("VirtioProxy"));
}

#[test]
fn the_founder_is_sent_down_the_wsl_path_first() {
    let advice = advise(&founder());
    assert_eq!(advice.path, VmPath::Wsl);
    // KVM first; packages wait until the next doctor run shows the svm flag.
    assert_eq!(
        lines(&advice),
        [
            r#"notepad "C:\Users\user\.wslconfig""#,
            "wsl --shutdown",
            "cargo xtask setup qwen3-4b-2507",
            "cargo xtask doctor",
        ]
    );
    assert!(advice.steps[0].say.contains("nestedVirtualization=true"));
    assert_eq!(
        advice.then,
        "cargo xtask run --accel wsl --local qwen3-4b-2507"
    );
}

#[test]
fn every_boot_command_the_doctor_gives_parses() {
    let ready = with_wsl(founder(), |w| w.probe = parse_report(READY_PROBE));
    let mut whpx = founder();
    whpx.wslconfig.nested = Some(true);
    let mut linux = founder();
    linux.windows = false;
    linux.wsl = None;
    linux.native = Some(parse_report(READY_PROBE));
    for facts in [founder(), ready, whpx, linux] {
        let then = advise(&facts).then;
        let words: Vec<String> = then.split(' ').skip(2).map(String::from).collect();
        assert!(then.starts_with("cargo xtask "), "{then}");
        assert!(crate::options::parse_args(&words).is_ok(), "{then}");
    }
}

#[test]
fn nested_set_without_a_flag_turns_to_whpx() {
    let mut facts = founder();
    facts.wslconfig.nested = Some(true);
    let advice = advise(&facts);
    assert_eq!(advice.path, VmPath::Whpx);
    let steps = lines(&advice);
    assert_eq!(
        steps[0],
        "winget install --id SoftwareFreedomConservancy.QEMU"
    );
    assert!(steps.iter().any(|s| s.starts_with("Dism /Online")));
    assert_eq!(
        advice.then,
        "cargo xtask run --accel whpx --local qwen3-4b-2507"
    );
    // The checklist agrees: the setting passes, and the flag points at WHPX.
    let list = items(&facts);
    assert!(item(&list, "nestedVirtualization").ok);
    assert!(item(&list, "svm/vmx flag").fix[2].say.contains("WHPX"));
    // Without WHPX on offer, WSL stays the path: update it and look again.
    facts.whpx = Feature::Absent;
    let advice = advise(&facts);
    assert_eq!(advice.path, VmPath::Wsl);
    assert_eq!(lines(&advice)[0], "wsl --update");
}

#[test]
fn a_ready_path_needs_no_steps() {
    let mut facts = with_wsl(founder(), |w| w.probe = parse_report(READY_PROBE));
    facts.store = Ok(store(Held::Verified));
    let advice = advise(&facts);
    assert_eq!((advice.path, advice.steps.len()), (VmPath::Wsl, 0));
    let failing: Vec<String> = items(&facts)
        .into_iter()
        .filter(|i| !i.ok)
        .map(|i| i.name)
        .collect();
    assert_eq!(failing, ["QEMU for Windows", "Windows Hypervisor Platform"]);
    // WHPX ready while WSL is not: WHPX now, with nothing to do.
    let mut whpx = founder();
    whpx.store = Ok(store(Held::Verified));
    whpx.whpx = Feature::On;
    whpx.qemu = Some(HostQemu {
        path: "C:\\Program Files\\qemu\\qemu-system-x86_64.exe".into(),
        version: "QEMU 9.2.0".into(),
    });
    let advice = advise(&whpx);
    assert_eq!((advice.path, advice.steps.len()), (VmPath::Whpx, 0));
}

#[test]
fn kvm_and_packages_get_their_fixes_once_the_flag_is_there() {
    let denied = with_wsl(founder(), |w| {
        w.probe = parse_report(&READY_PROBE.replace("kvm=usable", "kvm=denied"));
    });
    assert_eq!(
        lines(&advise(&denied))[..2],
        [
            "wsl -d Ubuntu -u root -- usermod -aG kvm dane1",
            "wsl --shutdown"
        ]
    );
    // The flag without /dev/kvm: WSL's kernel needs an update.
    let missing = with_wsl(founder(), |w| {
        w.probe = parse_report(&READY_PROBE.replace("kvm=usable", "kvm=missing"));
    });
    assert_eq!(lines(&advise(&missing))[0], "wsl --update");
    // KVM works and QEMU is there; OVMF and the window are not.
    let ovmf = "ovmf=/usr/share/OVMF/OVMF_CODE_4M.fd|/usr/share/OVMF/OVMF_VARS_4M.fd\n";
    let bare = READY_PROBE.replace(" gtk sdl", "").replace(ovmf, "");
    let partial = with_wsl(founder(), |w| w.probe = parse_report(&bare));
    let advice = advise(&partial);
    assert_eq!(
        lines(&advice)[1],
        "wsl -d Ubuntu -u root -- apt-get install -y qemu-system-gui ovmf"
    );
    assert!(
        advice.steps[0]
            .say
            .contains("qemu-system-gui, ovmf in Ubuntu")
    );
}

#[test]
fn hypervisor_and_missing_wsl_get_their_fixes() {
    let mut off = founder();
    off.hypervisor = Some(false);
    let advice = lines(&advise(&off));
    assert!(advice[0].contains("firmware (BIOS)"));
    assert_eq!(advice[1], "bcdedit /enum \"{current}\"");
    assert_eq!(advice[2], "bcdedit /set hypervisorlaunchtype auto");
    assert!(!item(&items(&off), "Hypervisor (hypervisorlaunchtype)").ok);
    // No WSL and no WHPX: a hypervisor that does not run is expected.
    let none = with_wsl(off, |w| {
        w.distro = Err(NoDistro::NotInstalled);
        w.probe = Err("not asked".into());
    });
    assert!(item(&items(&none), "Hypervisor (hypervisorlaunchtype)").ok);
    let advice = advise(&none);
    assert_eq!(advice.path, VmPath::Wsl);
    assert_eq!(lines(&advice)[0], "wsl --install -d Ubuntu-24.04");
    let unknown = with_wsl(founder(), |w| {
        w.distro = Err(NoDistro::NotListed("Debian".into(), vec!["Ubuntu".into()]));
    });
    assert_eq!(lines(&advise(&unknown))[0], "setx KAI_WSL_DISTRO Ubuntu");
    let mut unread = founder();
    unread.hypervisor = None;
    assert!(!item(&items(&unread), "Hypervisor (hypervisorlaunchtype)").ok);
}

#[test]
fn wsl_1_and_a_silent_distro_get_their_own_steps() {
    let old = with_wsl(founder(), |w| {
        w.distro = Ok(ubuntu(1));
        w.probe = Err("Ubuntu is WSL 1".into());
    });
    assert_eq!(
        lines(&advise(&old))[..2],
        [
            "wsl --set-version Ubuntu 2",
            "cargo xtask setup qwen3-4b-2507"
        ]
    );
    assert_eq!(items(&old).iter().filter(|i| i.name == "WSL").count(), 1);
    let silent = with_wsl(founder(), |w| {
        w.probe = Err("gave no answer in 240 s".into());
    });
    assert_eq!(
        lines(&advise(&silent))[..2],
        ["wsl --shutdown", "wsl -d Ubuntu"]
    );
    assert!(!item(&items(&silent), "WSL").ok);
}

#[test]
fn the_store_item_names_each_state() {
    let found = |held: Held| {
        let mut facts = founder();
        facts.store = Ok(store(held));
        item(&items(&facts), "Model store").clone()
    };
    let verified = found(Held::Verified);
    assert!(verified.ok && verified.found.contains("qwen3-4b-2507 verified"));
    let partial = found(Held::Partial(100_000_000));
    assert!(!partial.ok && partial.fix[0].say.contains("at 100 of 2497 MB"));
    let corrupt = found(Held::Corrupt("ab".into()));
    let delete = corrupt.fix[0].run.as_deref().unwrap();
    assert!(delete.starts_with("del \"D:\\kai-models") && delete.ends_with("Q4_K_M.gguf\""));
    assert_eq!(found(Held::WrongSize(5)).fix.len(), 2);
    let unreadable = found(Held::Unreadable("os error 32".into()));
    assert!(unreadable.fix[0].say.starts_with("Close any program"));
    // A store KAI_MODELS put on C: is fixed by pointing KAI_MODELS elsewhere.
    let mut broken = founder();
    broken.store = Err("C:\\m is on C:".into());
    broken.store_env = Some("C:\\m".into());
    let advice = advise(&broken);
    assert!(lines(&advice).contains(&"setx KAI_MODELS D:\\kai-models".to_string()));
    assert_eq!(advice.then, "cargo xtask run --accel wsl");
}

#[test]
fn a_linux_host_is_sent_to_kvm() {
    let mut facts = founder();
    facts.windows = false;
    facts.wsl = None;
    facts.wslconfig = WslConfig::default();
    facts.native = Some(parse_report(READY_PROBE));
    facts.store = Ok(store(Held::Verified));
    let advice = advise(&facts);
    assert_eq!((advice.path, advice.steps.len()), (VmPath::Kvm, 0));
    assert_eq!(
        advice.then,
        "cargo xtask run --accel kvm --local qwen3-4b-2507"
    );
    let list = items(&facts);
    assert!(item(&list, "/dev/kvm").ok && item(&list, "QEMU").ok);
    facts.native = Some(parse_report(FOUNDER_PROBE));
    let advice = lines(&advise(&facts));
    assert!(advice[0].contains("firmware (BIOS)"));
    assert_eq!(advice[1], "sudo apt-get update");
}

#[test]
fn the_model_follows_the_ram_as_setup_picks_it() {
    let inv = store(Held::Missing);
    let gb = |n: u64| Some(n << 30);
    assert_eq!(inv.recommend(gb(32)).unwrap().id, "qwen3-4b-2507");
    assert_eq!(
        inv.recommend(Some((8 << 30) - (1 << 28))).unwrap().id,
        "qwen3-4b-2507"
    );
    assert_eq!(inv.recommend(gb(6)).unwrap().id, "qwen3-1.7b");
    assert!(inv.recommend(gb(2)).is_none());
    assert_eq!(inv.recommend(None).unwrap().id, "qwen3-4b-2507");
    // The 0.6B alone, on a 32 GB PC: the store still asks for the 4B.
    let mut facts = founder();
    let mut small = store(Held::Missing);
    small.models[0].held = Held::Verified;
    facts.store = Ok(small);
    let list = items(&facts);
    let store_item = item(&list, "Model store");
    assert!(
        !store_item.ok
            && store_item
                .found
                .contains("qwen3-4b-2507, the one for this RAM")
    );
    assert!(
        item(&list, "RAM")
            .found
            .contains("Qwen3-4B-Instruct-2507 Q4_K_M (8 GB and up)")
    );
    assert_eq!(
        advise(&facts).then,
        "cargo xtask run --accel wsl --local qwen3-4b-2507"
    );
}

#[test]
fn the_vm_for_the_model_must_fit_in_wsl() {
    // The 4B's guest: 512 MiB, the 2.4 GB model and the engine's 640 MiB.
    assert_eq!(crate::disk::memory_mib_for(2_497_281_120), 3584);
    let list = items(&founder());
    let vm = item(&list, "VM size in WSL");
    assert!(vm.ok, "{vm:?}");
    assert!(
        vm.found
            .contains("3.5 GB guest with 8 vCPUs; WSL has 13.5 GB"),
        "{vm:?}"
    );
    let tight = with_wsl(founder(), |w| {
        w.probe = parse_report(&FOUNDER_PROBE.replace("mem_kib=14132508", "mem_kib=3900000"));
    });
    let list = items(&tight);
    let vm = item(&list, "VM size in WSL");
    assert!(!vm.ok);
    assert!(vm.fix[0].say.contains("put the line memory=6GB"), "{vm:?}");
}
