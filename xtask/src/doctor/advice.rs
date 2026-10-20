//! The doctor's verdict: one way to run Kernel AI accelerated on this PC, the
//! steps that get there in order, and the command that boots it.
//!
//! On Windows the choice is WSL2 with KVM, the path CI tests, unless it cannot
//! work: when WSL shows no svm/vmx flag although `nestedVirtualization=true` is
//! set, KVM cannot run inside it, and QEMU for Windows with WHPX is the way. A
//! path that is ready now wins over one that needs work.

use super::checks::{
    Facts, Feature, distro_restart, enable_whpx, hypervisor_fix, install_qemu_windows, replace,
    store_fix, wsl_fix,
};
use crate::models::Held;
use crate::wsl::check::{PACKAGES, Place, Step, firmware, run, say, step};
use crate::wsl::probe::{Kvm, Report};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmPath {
    Wsl,
    Whpx,
    Kvm,
}

impl VmPath {
    pub fn title(self) -> &'static str {
        match self {
            VmPath::Wsl => "WSL2 with KVM, the same QEMU and accelerator CI tests with",
            VmPath::Whpx => "QEMU for Windows with WHPX (CI does not test this path)",
            VmPath::Kvm => "QEMU with KVM on this Linux host, as CI runs it",
        }
    }

    fn accel(self) -> &'static str {
        match self {
            VmPath::Wsl => "wsl",
            VmPath::Whpx => "whpx",
            VmPath::Kvm => "kvm",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advice {
    pub path: VmPath,
    pub why: String,
    pub steps: Vec<Step>,
    /// The command that boots Kernel AI once the steps are done.
    pub then: String,
}

pub fn advise(f: &Facts) -> Advice {
    let (path, why) = choose(f);
    let mut steps = match path {
        VmPath::Wsl => wsl_steps(f),
        VmPath::Whpx => whpx_steps(f),
        VmPath::Kvm => kvm_steps(f),
    };
    // After a fix to the VM path, the doctor confirms it, or says what is next.
    let recheck = !steps.is_empty();
    steps.extend(model_steps(f));
    if recheck {
        steps.push(step(
            "Check again; the lines for this path should now pass:",
            "cargo xtask doctor",
        ));
    }
    let local = match &f.store {
        // The model setup picks for this RAM, else one already verified.
        Ok(inv) => match inv.recommend(f.ram_bytes) {
            Some(model) => format!(" --local {}", model.id),
            None => inv
                .models
                .iter()
                .find(|m| m.held == Held::Verified && m.runnable)
                .map_or_else(String::new, |m| format!(" --local {}", m.id)),
        },
        Err(_) => String::new(),
    };
    Advice {
        path,
        why,
        steps,
        then: format!("cargo xtask run --accel {}{local}", path.accel()),
    }
}

fn wsl_report(f: &Facts) -> Option<&Report> {
    let facts = f.wsl.as_ref()?;
    facts.distro.as_ref().ok().filter(|d| d.version == 2)?;
    facts.probe.as_ref().ok()
}

fn wsl_ready(r: &Report) -> bool {
    r.kvm == Kvm::Usable && r.qemu.is_some() && r.ovmf.is_some() && r.window()
}

fn choose(f: &Facts) -> (VmPath, String) {
    if !f.windows {
        return (VmPath::Kvm, "KVM is Linux's own accelerator.".into());
    }
    let report = wsl_report(f);
    let whpx_ready = f.qemu.is_some() && f.whpx == Feature::On && f.hypervisor != Some(false);
    let nested_set = f.wslconfig.nested == Some(true);
    match report {
        Some(r) if wsl_ready(r) => (VmPath::Wsl, "Everything it needs is here.".into()),
        _ if whpx_ready => (
            VmPath::Whpx,
            "QEMU for Windows and WHPX are ready, and WSL is not yet.".into(),
        ),
        Some(r) if r.virt_flags == 0 && nested_set && f.whpx != Feature::Absent => (
            VmPath::Whpx,
            "WSL shows no svm/vmx flag although nestedVirtualization=true is set, so KVM \
             cannot run inside it. (wsl --update, then wsl --shutdown, may still bring the \
             flag; the doctor then recommends WSL again.)"
                .into(),
        ),
        _ => (
            VmPath::Wsl,
            "It is what CI tests; the steps below get it running.".into(),
        ),
    }
}

fn hypervisor_steps(f: &Facts) -> Vec<Step> {
    let wanted = f.wsl_installed() || f.whpx == Feature::On;
    match f.hypervisor {
        Some(false) if wanted => hypervisor_fix(),
        _ => Vec::new(),
    }
}

fn wsl_steps(f: &Facts) -> Vec<Step> {
    let mut steps = hypervisor_steps(f);
    let Some(facts) = &f.wsl else {
        return steps;
    };
    let distro = match &facts.distro {
        Ok(distro) => distro,
        Err(missing) => {
            steps.extend(wsl_fix(missing));
            return steps;
        }
    };
    if distro.version == 1 {
        let convert = format!("wsl --set-version {} 2", distro.name);
        steps.push(step("Convert the distro to WSL 2:", convert));
        return steps;
    }
    match &facts.probe {
        Ok(report) => {
            let place = Place {
                distro: Some(&distro.name),
                config: &f.wslconfig,
            };
            steps.extend(linux_steps(report, place));
        }
        Err(_) => steps.extend(distro_restart(&distro.name)),
    }
    steps
}

/// KVM first; then, once it works, QEMU's packages and its window.
fn linux_steps(r: &Report, place: Place) -> Vec<Step> {
    let mut steps = Vec::new();
    match (r.virt_flags, r.kvm, place.distro) {
        (0, _, None) => steps.push(firmware()),
        // The next doctor run says whether the flag came, and so which path to take.
        (0, ..) => return place.virt_fix(),
        (_, Kvm::Denied, _) => {
            steps.push(run(place.root(&format!("usermod -aG kvm {}", r.user))));
            steps.push(place.restart());
        }
        (_, Kvm::Missing, _) => steps.extend(place.kvm_fix()),
        (_, Kvm::Usable, _) => {}
    }
    let gui = r.displays.iter().any(|d| d == "gtk" || d == "sdl");
    let packages: Vec<&str> = match &r.qemu {
        None => PACKAGES.split(' ').collect(),
        Some(_) => [
            (!gui && place.distro.is_some()).then_some("qemu-system-gui"),
            r.ovmf.is_none().then_some("ovmf"),
        ]
        .into_iter()
        .flatten()
        .collect(),
    };
    if !packages.is_empty() {
        let what = match place.distro {
            Some(distro) => format!("Install {} in {distro}:", packages.join(", ")),
            None => format!("Install {}:", packages.join(", ")),
        };
        let mut install = place.install(&packages.join(" "));
        install[0].say = what;
        steps.extend(install);
    }
    if gui && !r.has_display && place.distro.is_some() {
        steps.extend(place.display_fix(r));
    }
    steps
}

fn whpx_steps(f: &Facts) -> Vec<Step> {
    let mut steps = hypervisor_steps(f);
    if f.qemu.is_none() {
        steps.extend(install_qemu_windows());
    }
    if f.whpx != Feature::On {
        steps.extend(enable_whpx());
    }
    steps
}

fn kvm_steps(f: &Facts) -> Vec<Step> {
    let place = Place {
        distro: None,
        config: &f.wslconfig,
    };
    match &f.native {
        Some(Ok(report)) => linux_steps(report, place),
        _ => Vec::new(),
    }
}

/// What gets the model `setup` picks for this RAM (the 4B from 8 GB) into the
/// store, unless it is verified there already.
fn model_steps(f: &Facts) -> Vec<Step> {
    let inv = match &f.store {
        Ok(inv) => inv,
        Err(_) => return store_fix(f.windows, f.store_env.as_deref()),
    };
    let Some(model) = inv.recommend(f.ram_bytes) else {
        return Vec::new();
    };
    let setup = format!("cargo xtask setup {}", model.id);
    match &model.held {
        Held::Verified => Vec::new(),
        Held::Missing | Held::Partial(_) => vec![step(
            format!(
                "Download {} ({} MB) into {}:",
                model.name,
                model.size / 1_000_000,
                inv.dir.display()
            ),
            setup,
        )],
        Held::WrongSize(_) | Held::Corrupt(_) => {
            replace(f.windows, &inv.dir.join(&model.file), &setup)
        }
        Held::Unreadable(_) => vec![say(format!(
            "Close any program using {}, then run the doctor again.",
            model.file
        ))],
    }
}
