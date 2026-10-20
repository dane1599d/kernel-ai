//! The doctor's checklist, worked out from facts gathered beforehand, so every
//! decision is a pure function tested over canned outputs. Each item passes or
//! fails, and a failure carries the steps that fix it. The Linux side's items
//! are `wsl::check`'s, the ones `--accel wsl` refuses a boot with.

use crate::models::{Held, Inventory, Stocked};
use crate::options::MAX_VM_CPUS;
use crate::wsl::HEADROOM_MIB;
use crate::wsl::check::{
    Item, Needs, Place, Step, WslConfig, fail, firmware, linux, nested, pass, run, say, step,
    wslconfig_steps,
};
use crate::wsl::probe::{Distro, NoDistro, Report};

/// Less free space than this on a drive is flagged.
pub const MIN_FREE_BYTES: u64 = 10 << 30;
/// Less RAM than this is flagged: the default model's VM, Windows and WSL. Firmware
/// and integrated graphics take some, so 7.5 GiB visible still counts as 8.
pub const MIN_RAM_BYTES: u64 = (8 << 30) - (1 << 29);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    On,
    Off,
    /// Windows does not offer it.
    Absent,
    Unknown,
}

impl Feature {
    /// From `Win32_OptionalFeature.InstallState`: 1 enabled, 2 disabled, 3 absent.
    pub fn from_install_state(state: &str) -> Self {
        match state.trim() {
            "1" => Feature::On,
            "2" => Feature::Off,
            "3" => Feature::Absent,
            _ => Feature::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    /// `C:`, or a folder on a host without drive letters.
    pub place: String,
    pub free: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostQemu {
    pub path: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslFacts {
    /// `wsl --version`'s first line.
    pub version: Option<String>,
    pub distro: Result<Distro, NoDistro>,
    pub probe: Result<Report, String>,
    /// What WSL printed on stderr while starting, such as a network fallback.
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub windows: bool,
    pub avx2_fma: bool,
    pub avx512: bool,
    pub ram_bytes: Option<u64>,
    pub space: Vec<Space>,
    pub store: Result<Inventory, String>,
    /// `KAI_MODELS`, when set: it chooses the store.
    pub store_env: Option<String>,
    /// QEMU for Windows, when installed and answering `--version`.
    pub qemu: Option<HostQemu>,
    pub whpx: Feature,
    /// Whether Windows' hypervisor runs, which hypervisorlaunchtype Off prevents.
    pub hypervisor: Option<bool>,
    pub wsl: Option<WslFacts>,
    pub wslconfig: WslConfig,
    /// The probe run on a Linux host itself.
    pub native: Option<Result<Report, String>>,
}

impl Facts {
    pub fn wsl_installed(&self) -> bool {
        self.wsl
            .as_ref()
            .is_some_and(|w| w.distro != Err(NoDistro::NotInstalled))
    }
}

pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / f64::from(1u32 << 30))
}

pub fn install_qemu_windows() -> Vec<Step> {
    vec![
        step(
            "Install QEMU for Windows:",
            "winget install --id SoftwareFreedomConservancy.QEMU",
        ),
        say("Then open a new terminal, so that its PATH has QEMU."),
    ]
}

pub fn enable_whpx() -> Vec<Step> {
    vec![
        say(
            "Start → type \"Turn Windows features on or off\" → tick Windows Hypervisor \
             Platform → OK → restart Windows.",
        ),
        step(
            "Or in an administrator terminal, then restart Windows:",
            "Dism /Online /Enable-Feature /FeatureName:HypervisorPlatform /All",
        ),
    ]
}

/// Windows' hypervisor does not start: virtualization is off in the firmware, or
/// hypervisorlaunchtype is Off, which only an administrator can read.
pub fn hypervisor_fix() -> Vec<Step> {
    vec![
        firmware(),
        step(
            "Then, in an administrator terminal, read hypervisorlaunchtype:",
            "bcdedit /enum \"{current}\"",
        ),
        step(
            "If it says Off, set it back, then restart Windows:",
            "bcdedit /set hypervisorlaunchtype auto",
        ),
    ]
}

pub fn wsl_fix(missing: &NoDistro) -> Vec<Step> {
    match missing {
        NoDistro::NotInstalled | NoDistro::NoneListed => vec![step(
            "Install WSL with Ubuntu, restart Windows, then create a user when Ubuntu asks:",
            "wsl --install -d Ubuntu-24.04",
        )],
        NoDistro::ListFailed(_) => vec![
            step("Restart WSL:", "wsl --shutdown"),
            step("Then see what it lists:", "wsl -l -v"),
        ],
        NoDistro::NotListed(_, listed) => {
            let usable = listed.iter().find(|n| !n.starts_with("docker-desktop"));
            match usable {
                Some(name) => vec![
                    step(
                        "Point KAI_WSL_DISTRO at a listed distro:",
                        format!("setx KAI_WSL_DISTRO {name}"),
                    ),
                    say("Then open a new terminal."),
                ],
                None => wsl_fix(&NoDistro::NoneListed),
            }
        }
    }
}

/// A store that cannot be used: `KAI_MODELS` names one on C: or in OneDrive.
pub fn store_fix(windows: bool, env: Option<&str>) -> Vec<Step> {
    match (env, windows) {
        (Some(value), true) => vec![
            step(
                format!("KAI_MODELS is {value}; point it off C: and outside OneDrive:"),
                "setx KAI_MODELS D:\\kai-models",
            ),
            say("Then open a new terminal."),
        ],
        (Some(value), false) => vec![step(
            format!("KAI_MODELS is {value}; point it elsewhere:"),
            "export KAI_MODELS=~/kai-models",
        )],
        (None, true) => vec![run("cargo xtask setup --store D:\\kai-models")],
        (None, false) => vec![run("cargo xtask setup --store ~/kai-models")],
    }
}

pub fn items(f: &Facts) -> Vec<Item> {
    let mut items = vec![cpu(f), ram(f)];
    items.extend(f.space.iter().map(space));
    items.push(store(f));
    if f.windows {
        items.push(windows_qemu(f.qemu.as_ref()));
        items.push(whpx(f.whpx));
        items.push(hypervisor(f));
        items.extend(wsl(f));
    } else if let Some(native) = &f.native {
        let place = Place {
            distro: None,
            config: &f.wslconfig,
        };
        let needs = Needs {
            window: false,
            loopback: false,
        };
        match native {
            Ok(report) => items.extend(linux(report, place, needs)),
            Err(e) => items.push(fail("Linux probe", e.clone(), Vec::new())),
        }
    }
    items
}

fn cpu(f: &Facts) -> Item {
    let avx512 = if f.avx512 { "; AVX-512" } else { "" };
    if f.avx2_fma {
        pass(
            "CPU",
            format!("AVX2 and FMA, the engine's fast tier{avx512}"),
        )
    } else {
        fail(
            "CPU",
            "no AVX2 and FMA: the engine runs its baseline tier, about 4x slower",
            vec![say("None in software: the fast tier needs AVX2 and FMA.")],
        )
    }
}

fn ram(f: &Facts) -> Item {
    match f.ram_bytes {
        Some(bytes) if bytes >= MIN_RAM_BYTES => {
            let model = f
                .store
                .as_ref()
                .ok()
                .and_then(|inv| inv.recommend(Some(bytes)));
            let tier = model.map_or_else(String::new, |m| {
                format!("; enough for {} ({} GB and up)", m.name, m.min_ram_gb)
            });
            pass("RAM", format!("{} usable{tier}", gb(bytes)))
        }
        Some(bytes) => fail(
            "RAM",
            format!(
                "{}, below the 8 GB a VM with the model and the host need",
                gb(bytes)
            ),
            vec![say("Close other programs before a boot with the model.")],
        ),
        None => fail("RAM", "could not be read", Vec::new()),
    }
}

fn space(s: &Space) -> Item {
    let name = format!("Free on {}", s.place);
    match s.free {
        Some(free) if free >= MIN_FREE_BYTES => pass(&name, gb(free)),
        Some(free) => fail(
            &name,
            format!("{}, below the 10 GB builds and models want", gb(free)),
            vec![say(if s.place == "C:" {
                "Settings → System → Storage → Cleanup recommendations.".to_string()
            } else {
                format!("Free some space on {}.", s.place)
            })],
        ),
        None => fail(&name, "could not be read", Vec::new()),
    }
}

fn store(f: &Facts) -> Item {
    let name = "Model store";
    let inv = match &f.store {
        Ok(inv) => inv,
        Err(e) => {
            return fail(
                name,
                e.clone(),
                store_fix(f.windows, f.store_env.as_deref()),
            );
        }
    };
    let dir = inv.dir.display();
    let ready: Vec<&str> = inv
        .models
        .iter()
        .filter(|m| m.held == Held::Verified)
        .map(|m| m.id.as_str())
        .collect();
    // The model setup picks for this RAM: the 4B from 8 GB.
    let want = inv.recommend(f.ram_bytes).map(|m| m.id.as_str());
    let mut fix = Vec::new();
    let mut bad = Vec::new();
    for model in &inv.models {
        let setup = format!("cargo xtask setup {}", model.id);
        let mb = |bytes: u64| bytes / 1_000_000;
        match &model.held {
            Held::Verified => {}
            Held::Missing if Some(model.id.as_str()) == want => fix.push(step(
                format!(
                    "Download {} ({} MB) and check its SHA-256:",
                    model.name,
                    mb(model.size)
                ),
                setup,
            )),
            Held::Missing => {}
            Held::Partial(have) => {
                bad.push(format!("{} partly downloaded", model.id));
                fix.push(step(
                    format!(
                        "Resume the download at {} of {} MB:",
                        mb(*have),
                        mb(model.size)
                    ),
                    setup,
                ));
            }
            Held::WrongSize(len) => {
                bad.push(format!("{} has {len} bytes, not {}", model.id, model.size));
                fix.extend(replace(f.windows, &inv.dir.join(&model.file), &setup));
            }
            Held::Corrupt(_) => {
                bad.push(format!(
                    "{} is not the pinned file (SHA-256 differs)",
                    model.id
                ));
                fix.extend(replace(f.windows, &inv.dir.join(&model.file), &setup));
            }
            Held::Unreadable(e) => {
                bad.push(format!("{}: {e}", model.id));
                fix.push(say(format!(
                    "Close any program using {}, then run the doctor again.",
                    model.file
                )));
            }
        }
    }
    let missing = want.filter(|id| !ready.contains(id));
    if let (Some(id), false) = (missing, ready.is_empty()) {
        bad.push(format!("{id}, the one for this RAM, is not downloaded"));
    }
    let found = match (ready.is_empty(), bad.is_empty()) {
        (false, true) => format!("{dir}: {} verified (size and SHA-256)", ready.join(", ")),
        (false, false) => format!("{dir}: {} verified; {}", ready.join(", "), bad.join("; ")),
        (true, true) => format!("{dir}: no catalog model downloaded yet"),
        (true, false) => format!("{dir}: {}", bad.join("; ")),
    };
    Item {
        ok: !ready.is_empty() && bad.is_empty() && missing.is_none(),
        name: name.into(),
        found,
        fix,
    }
}

/// Deleting a wrong model file and downloading the pinned one. `del` works in
/// cmd and in PowerShell alike.
pub fn replace(windows: bool, path: &std::path::Path, setup: &str) -> Vec<Step> {
    let delete = match windows {
        true => format!("del \"{}\"", path.display()),
        false => format!("rm \"{}\"", path.display()),
    };
    vec![step("Delete the wrong file:", delete), run(setup)]
}

fn windows_qemu(qemu: Option<&HostQemu>) -> Item {
    let name = "QEMU for Windows";
    match qemu {
        Some(q) => pass(name, format!("{} at {}", q.version, q.path)),
        None => fail(
            name,
            "not installed (needed only for the WHPX path)",
            install_qemu_windows(),
        ),
    }
}

/// The Windows Hypervisor Platform, WHPX, read through CIM without an administrator.
fn whpx(state: Feature) -> Item {
    let name = "Windows Hypervisor Platform";
    match state {
        Feature::On => pass(name, "on (QEMU for Windows can use -accel whpx)"),
        Feature::Off => fail(name, "off (needed only for the WHPX path)", enable_whpx()),
        Feature::Absent => fail(name, "not offered by this Windows", Vec::new()),
        Feature::Unknown => fail(name, "could not be read", enable_whpx()),
    }
}

/// hypervisorlaunchtype needs an administrator's bcdedit to read, but its effect
/// does not: Windows' hypervisor runs when WSL2 or WHPX is on and it is not Off.
fn hypervisor(f: &Facts) -> Item {
    let name = "Hypervisor (hypervisorlaunchtype)";
    let wanted = f.wsl_installed() || f.whpx == Feature::On;
    match f.hypervisor {
        Some(true) => pass(name, "Windows' hypervisor is running, so it is not Off"),
        Some(false) if !wanted => pass(name, "not running yet; installing WSL or WHPX starts it"),
        Some(false) => fail(
            name,
            "not running, though WSL or WHPX needs it: virtualization is off in the firmware, \
             or hypervisorlaunchtype is Off",
            hypervisor_fix(),
        ),
        None => fail(name, "could not be read", hypervisor_fix()),
    }
}

fn wsl(f: &Facts) -> Vec<Item> {
    let Some(facts) = &f.wsl else {
        return Vec::new();
    };
    let distro = match &facts.distro {
        Ok(distro) => distro,
        Err(missing) => return vec![fail("WSL", missing.describe(), wsl_fix(missing))],
    };
    if distro.version == 1 {
        let convert = step(
            "Convert it to WSL 2:",
            format!("wsl --set-version {} 2", distro.name),
        );
        let found = format!("{} runs as WSL 1, which has no KVM", distro.name);
        return vec![fail("WSL", found, vec![convert])];
    }
    let report = match &facts.probe {
        Ok(report) => report,
        Err(e) => {
            let found = format!("{} did not answer: {e}", distro.name);
            return vec![fail("WSL", found, distro_restart(&distro.name))];
        }
    };
    let inside = match (report.mem_kib, report.cpus) {
        (Some(kib), Some(cpus)) => format!(", {} and {cpus} CPUs inside", gb(kib * 1024)),
        _ => String::new(),
    };
    let version = facts.version.as_deref().unwrap_or("WSL");
    let mut found = format!("{} (WSL 2; {version}){inside}", distro.name);
    if let Some(note) = &facts.note {
        found.push_str(&format!("; WSL says: {note}"));
    }
    let mut items = vec![pass("WSL", found), nested(&f.wslconfig, report.virt_flags)];
    let place = Place {
        distro: Some(&distro.name),
        config: &f.wslconfig,
    };
    let needs = Needs {
        window: true,
        loopback: true,
    };
    items.extend(linux(report, place, needs));
    if let Some(model) = f.store.as_ref().ok().and_then(|i| i.recommend(f.ram_bytes)) {
        items.push(vm_memory(model, report, &f.wslconfig));
    }
    items
}

/// Whether WSL's VM holds the guest `run --local` gives the recommended model,
/// as `--accel wsl` checks before a boot.
fn vm_memory(model: &Stocked, report: &Report, config: &WslConfig) -> Item {
    let name = "VM size in WSL";
    let cpus = report
        .cpus
        .map_or(MAX_VM_CPUS, |n| (n as usize).min(MAX_VM_CPUS));
    let vm = format!(
        "{} gets a {} guest with {cpus} vCPUs",
        model.name,
        gb(model.vm_mib << 20)
    );
    let Some(kib) = report.mem_kib else {
        return pass(name, vm);
    };
    let wsl_mib = kib / 1024;
    if model.vm_mib + HEADROOM_MIB <= wsl_mib {
        return pass(name, format!("{vm}; WSL has {}", gb(kib * 1024)));
    }
    let want_gb = (model.vm_mib + HEADROOM_MIB).div_ceil(1024) + 1;
    fail(
        name,
        format!("{vm}, but WSL has only {}", gb(kib * 1024)),
        wslconfig_steps(config, &format!("memory={want_gb}GB")),
    )
}

/// A distro that did not answer: restart WSL, then let the distro show its error.
pub fn distro_restart(name: &str) -> Vec<Step> {
    vec![
        step("Restart WSL:", "wsl --shutdown"),
        step(
            "Then start the distro to see its own error:",
            format!("wsl -d {name}"),
        ),
    ]
}
