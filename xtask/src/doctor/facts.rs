//! Gathering the doctor's facts. Every probe only reads: one PowerShell query
//! (CIM and drive space, no administrator needed), `wsl.exe -l -v`, `wsl.exe
//! --version`, the probe script inside the WSL distro (which starts it if it was
//! stopped), `qemu-system-x86_64 --version`, `.wslconfig`, and the model store's
//! files (hashed, never changed). The parsers are pure and tested.

use super::checks::{Facts, Feature, HostQemu, Space, WslFacts};
use crate::models;
use crate::wsl::check::{WslConfig, qemu_version, read_wslconfig};
use crate::wsl::probe::{self, NoDistro, SHORT_LIMIT};
use std::collections::HashMap;
use std::process::Command;

pub fn gather() -> Facts {
    let cpu = cpufeat::Features::detect();
    let store = models::inventory(None);
    let mut facts = Facts {
        windows: cfg!(windows),
        avx2_fma: cpu.avx2_fma(),
        avx512: cpu.avx512(),
        ram_bytes: None,
        space: Vec::new(),
        store,
        store_env: std::env::var(models::STORE_ENV)
            .ok()
            .filter(|v| !v.is_empty()),
        qemu: None,
        whpx: Feature::Unknown,
        hypervisor: None,
        wsl: None,
        wslconfig: WslConfig::default(),
        native: None,
    };
    if facts.windows {
        windows(&mut facts);
    } else {
        unix(&mut facts);
    }
    facts
}

fn windows(facts: &mut Facts) {
    let store_drive = facts
        .store
        .as_ref()
        .ok()
        .and_then(|inv| drive_letter(&inv.dir.to_string_lossy()));
    let mut drives = vec!['C', 'D'];
    drives.extend(store_drive.filter(|d| !drives.contains(d)));
    let values = powershell(&windows_query(&drives)).map_or_else(HashMap::new, |t| keyvals(&t));
    facts.ram_bytes = values.get("ram").and_then(|v| v.parse().ok());
    facts.hypervisor = values.get("hypervisor").and_then(|v| match v.as_str() {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    });
    facts.whpx = values
        .get("whpx")
        .map_or(Feature::Unknown, |v| Feature::from_install_state(v));
    for drive in drives {
        let free = values
            .get(&format!("free_{drive}"))
            .and_then(|v| v.parse().ok());
        // C: always; another drive only when it exists.
        if drive == 'C' || free.is_some() {
            facts.space.push(Space {
                place: format!("{drive}:"),
                free,
            });
        }
    }
    // Found only if it answers: QEMU may name a file that is gone.
    facts.qemu = crate::find_qemu().ok().and_then(|path| {
        let out = probe::run(Command::new(&path).arg("--version"), SHORT_LIMIT).ok()?;
        Some(HostQemu {
            version: qemu_version(out.stdout.lines().next()?)?,
            path: path.display().to_string(),
        })
    });
    facts.wslconfig = read_wslconfig();
    facts.wsl = Some(wsl());
}

fn wsl() -> WslFacts {
    // Never wsl.exe without WSL: System32's stub would offer to install it.
    let distro = probe::find();
    let (probe, note) = match &distro {
        Ok(d) if d.version == 2 => {
            println!(
                "xtask: asking {} in WSL (a first start can take a minute or two)",
                d.name
            );
            match probe::ask(&d.name) {
                Ok((report, note)) => (Ok(report), note),
                Err(e) => (Err(e), None),
            }
        }
        Ok(d) => (Err(format!("{} is WSL 1", d.name)), None),
        Err(e) => (Err(e.describe()), None),
    };
    let installed = distro.as_ref().err() != Some(&NoDistro::NotInstalled);
    WslFacts {
        version: installed.then(probe::version).flatten(),
        distro,
        probe,
        note,
    }
}

fn unix(facts: &mut Facts) {
    let native = probe::ask_native();
    facts.ram_bytes = native
        .as_ref()
        .ok()
        .and_then(|r| r.mem_kib)
        .map(|kib| kib * 1024);
    if let Ok(inv) = &facts.store {
        facts.space.push(Space {
            place: inv.dir.display().to_string(),
            free: models::free_bytes(&inv.dir),
        });
    }
    let target = crate::workspace_root().join("target");
    facts.space.push(Space {
        place: target.display().to_string(),
        free: models::free_bytes(&target),
    });
    facts.native = Some(native);
}

/// The PowerShell that reads RAM, whether the hypervisor runs, the Windows
/// Hypervisor Platform's install state, and free bytes per drive.
pub fn windows_query(drives: &[char]) -> String {
    let drives: Vec<String> = drives.iter().map(|d| format!("'{d}'")).collect();
    [
        "$ErrorActionPreference = 'SilentlyContinue'",
        "$cs = Get-CimInstance Win32_ComputerSystem",
        "\"ram=$($cs.TotalPhysicalMemory)\"",
        "\"hypervisor=$($cs.HypervisorPresent)\"",
        "$f = Get-CimInstance Win32_OptionalFeature -Filter \"Name='HypervisorPlatform'\"",
        "\"whpx=$($f.InstallState)\"",
        &format!(
            "foreach ($d in {}) {{ $p = Get-PSDrive -Name $d -PSProvider FileSystem; \
             if ($p) {{ \"free_$d=$($p.Free)\" }} }}",
            drives.join(",")
        ),
        // A drive that is missing leaves an error behind, and so exit status 1.
        "exit 0",
    ]
    .join("\n")
}

/// Runs a script in Windows PowerShell, passed base64-encoded so no quoting
/// rule can change it. Whatever it printed is used, whatever its exit status.
fn powershell(script: &str) -> Option<String> {
    let out = probe::run(
        Command::new("powershell").args([
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encode_command(script),
        ]),
        SHORT_LIMIT,
    )
    .ok()?;
    Some(out.stdout)
}

/// What `-EncodedCommand` takes: the script in UTF-16LE, in base64.
pub fn encode_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `key=value` lines; later keys win.
pub fn keyvals(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.trim().split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// The drive letter of a Windows path such as `D:\kai-models`.
pub fn drive_letter(path: &str) -> Option<char> {
    let path = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let mut chars = path.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            Some(letter.to_ascii_uppercase())
        }
        _ => None,
    }
}
