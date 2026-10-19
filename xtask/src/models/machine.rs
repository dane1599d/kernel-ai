//! The PC check `setup` starts with: CPU features, RAM, free space for the store,
//! and whether QEMU and an accelerator are there to run Kernel AI itself. Each
//! gap comes with the exact command or setting that fixes it.

use std::path::Path;
use std::process::{Command, Stdio};

/// One line of the check: what was found, and the fix if something is missing.
pub struct Finding {
    pub topic: &'static str,
    pub found: String,
    pub fix: Option<String>,
}

pub struct Machine {
    pub findings: Vec<Finding>,
    pub ram_gb: Option<u64>,
    pub free_bytes: Option<u64>,
}

/// Checks the PC for a store at `store`.
pub fn check(store: &Path) -> Machine {
    let mut findings = vec![cpu()];
    let ram = ram_bytes();
    let ram_gb = ram.map(|b| (b + (1 << 29)) >> 30);
    findings.push(Finding {
        topic: "RAM",
        found: ram_gb.map_or("unknown".into(), |gb| format!("{gb} GB")),
        fix: None,
    });
    let free_bytes = free_bytes(store);
    findings.push(Finding {
        topic: "Store",
        found: match free_bytes {
            Some(free) => format!("{}, {} GB free", store.display(), free >> 30),
            None => format!("{}, free space unknown", store.display()),
        },
        fix: None,
    });
    findings.push(qemu());
    findings.push(accelerator());
    Machine {
        findings,
        ram_gb,
        free_bytes,
    }
}

fn cpu() -> Finding {
    let cpu = cpufeat::Features::detect();
    let avx512 = if cpu.avx512() {
        "AVX-512 yes (used from L6)"
    } else {
        "no AVX-512"
    };
    if cpu.avx2_fma() {
        Finding {
            topic: "CPU",
            found: format!("AVX2 and FMA yes, the engine's fast tier; {avx512}"),
            fix: None,
        }
    } else {
        Finding {
            topic: "CPU",
            found: "no AVX2 and FMA: the engine runs its baseline tier, about 4x slower".into(),
            fix: Some(
                "none in software; the fast tier needs AVX2 and FMA (most Core and Ryzen \
                 CPUs since 2015)"
                    .into(),
            ),
        }
    }
}

/// Runs a short command and returns its trimmed stdout if it succeeded.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn powershell(script: &str) -> Option<String> {
    output(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    )
}

fn ram_bytes() -> Option<u64> {
    if cfg!(windows) {
        powershell("(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory")?
            .parse()
            .ok()
    } else if cfg!(target_os = "macos") {
        output("sysctl", &["-n", "hw.memsize"])?.parse().ok()
    } else {
        let info = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = info
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))?
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()?;
        Some(kb * 1024)
    }
}

/// Free bytes on the drive or file system that holds `store` (or would).
pub fn free_bytes(store: &Path) -> Option<u64> {
    // The real folder, so a junction to another drive is measured there.
    let existing = store.ancestors().find_map(|p| p.canonicalize().ok())?;
    if cfg!(windows) {
        let text = existing.to_string_lossy();
        let text = text.strip_prefix("\\\\?\\").unwrap_or(&text);
        let mut chars = text.chars();
        let (Some(letter), Some(':')) = (chars.next(), chars.next()) else {
            return None;
        };
        if !letter.is_ascii_alphabetic() {
            return None;
        }
        powershell(&format!("(Get-PSDrive -Name {letter}).Free"))?
            .parse()
            .ok()
    } else {
        let path = existing.to_str()?;
        let out = output("df", &["-Pk", path])?;
        let kb: u64 = out
            .lines()
            .nth(1)?
            .split_whitespace()
            .nth(3)?
            .parse()
            .ok()?;
        Some(kb * 1024)
    }
}

fn qemu() -> Finding {
    let later = "QEMU runs Kernel AI itself (after L4); `model chat` does not need it";
    match crate::find_qemu() {
        Ok(path) => Finding {
            topic: "QEMU",
            found: format!("found at {}", path.display()),
            fix: None,
        },
        Err(_) if cfg!(windows) => Finding {
            topic: "QEMU",
            found: format!("not found. {later}"),
            fix: Some(
                "winget install --id SoftwareFreedomConservancy.QEMU, or in WSL: \
                 sudo apt-get install -y qemu-system-x86 ovmf"
                    .into(),
            ),
        },
        Err(_) => Finding {
            topic: "QEMU",
            found: format!("not found. {later}"),
            fix: Some("sudo apt-get install -y qemu-system-x86 ovmf".into()),
        },
    }
}

fn accelerator() -> Finding {
    if !cfg!(windows) {
        let kvm = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .is_ok();
        return Finding {
            topic: "Accel",
            found: if kvm {
                "KVM".into()
            } else {
                "no usable /dev/kvm".into()
            },
            fix: (!kvm).then(|| {
                "enable virtualisation (SVM or VT-x) in the firmware, then \
                 sudo usermod -aG kvm $USER and log in again"
                    .into()
            }),
        };
    }
    // KVM inside WSL2 is CI's path; WHPX is a measured fallback.
    let probe = "if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then echo kvm; \
                 elif [ -e /dev/kvm ]; then echo denied; else echo none; fi";
    let wsl = output("wsl.exe", &["-e", "sh", "-c", probe]);
    let kvm = wsl.as_deref() == Some("kvm");
    // The feature's state, not its DLL, which Windows keeps even when it is off.
    let whpx = powershell(
        "(Get-CimInstance Win32_OptionalFeature -Filter \"Name='HypervisorPlatform'\").InstallState",
    )
    .as_deref()
        == Some("1");
    let found = match (kvm, wsl.as_deref(), whpx) {
        (true, ..) => "KVM in WSL2 (the tested path)".to_string(),
        (false, Some("denied"), _) => "/dev/kvm in WSL2, but your WSL user cannot open it".into(),
        (false, _, true) => "WHPX only (on; untested here, with known SMP bugs)".to_string(),
        _ => "none: Kernel AI would run under TCG, minutes per answer".to_string(),
    };
    let fix = (!kvm).then(|| {
        let nested = "for KVM in WSL2: add nestedVirtualization=true under [wsl2] in \
                      %UserProfile%\\.wslconfig, run wsl --shutdown, and check that \
                      `grep -c svm /proc/cpuinfo` in WSL is above 0 (SVM must be on in the firmware)";
        // An installed distro that does not start has a different cure.
        let installed = output("wsl.exe", &["-l", "-q"]).is_some_and(|names| {
            names.chars().any(|c| c.is_ascii_alphanumeric())
        });
        let mut fix = match wsl.as_deref() {
            Some("denied") => "in WSL: sudo usermod -aG kvm $USER, then wsl --shutdown".to_string(),
            Some(_) => nested.to_string(),
            None if installed => "WSL is installed but did not start: run \
                 bcdedit /enum | findstr -i hypervisorlaunchtype, and if it is Off, \
                 bcdedit /set hypervisorlaunchtype auto (as administrator), then restart"
                .to_string(),
            None => format!("install WSL2 first: wsl --install -d Ubuntu-24.04; then {nested}"),
        };
        if !whpx {
            fix.push_str(
                "; or for WHPX: Dism /Online /Enable-Feature /FeatureName:HypervisorPlatform /All \
                 (as administrator), then restart",
            );
        }
        fix
    });
    Finding {
        topic: "Accel",
        found,
        fix,
    }
}
