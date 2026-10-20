//! The Linux side of a VM, as a checklist: the WSL2 distro seen from Windows, or
//! the Linux host itself. `--accel wsl` refuses a boot with these items, and
//! `cargo xtask doctor` shows them, so the fixes are the same in both. Pure
//! functions over the probe's report.

use super::probe::{Kvm, Report};

/// What `apt-get` installs for `--accel wsl`: QEMU, its window, and OVMF.
pub const PACKAGES: &str = "qemu-system-x86 qemu-system-gui ovmf";

/// One thing to do: what and why in words, then the command or the setting,
/// alone on its own line so it can be pasted as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub say: String,
    pub run: Option<String>,
}

/// A command, with nothing to say about it.
pub fn run(command: impl Into<String>) -> Step {
    Step {
        say: String::new(),
        run: Some(command.into()),
    }
}

/// Words alone: a menu path, or something to check.
pub fn say(text: impl Into<String>) -> Step {
    Step {
        say: text.into(),
        run: None,
    }
}

pub fn step(text: impl Into<String>, command: impl Into<String>) -> Step {
    Step {
        say: text.into(),
        run: Some(command.into()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub ok: bool,
    pub name: String,
    pub found: String,
    pub fix: Vec<Step>,
}

pub fn pass(name: &str, found: impl Into<String>) -> Item {
    Item {
        ok: true,
        name: name.into(),
        found: found.into(),
        fix: Vec::new(),
    }
}

pub fn fail(name: &str, found: impl Into<String>, fix: Vec<Step>) -> Item {
    Item {
        ok: false,
        name: name.into(),
        found: found.into(),
        fix,
    }
}

/// The checklist, names in one column; under a ✗, each step on its lines.
pub fn print_items(items: &[Item]) {
    let width = items
        .iter()
        .map(|i| i.name.chars().count())
        .max()
        .unwrap_or(0);
    for item in items {
        let mark = if item.ok { '✓' } else { '✗' };
        println!("  {mark} {:<width$}  {}", item.name, item.found);
        for step in &item.fix {
            print_step("      → ", "          ", step);
        }
    }
}

/// A step's words after `lead`, and its command on the next line after `indent`,
/// or on the same line when there are no words.
pub fn print_step(lead: &str, indent: &str, step: &Step) {
    match (&step.say, &step.run) {
        (say, Some(run)) if say.is_empty() => println!("{lead}{run}"),
        (say, Some(run)) => {
            println!("{lead}{say}");
            println!("{indent}{run}");
        }
        (say, None) => println!("{lead}{say}"),
    }
}

/// What the doctor and `--accel wsl` read from `%UserProfile%\.wslconfig`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WslConfig {
    pub path: String,
    pub exists: bool,
    /// Whether the file has a `[wsl2]` section.
    pub has_wsl2: bool,
    /// `nestedVirtualization` under `[wsl2]`, when set.
    pub nested: Option<bool>,
    /// `networkingMode` under `[wsl2]`, when set.
    pub networking: Option<String>,
}

/// Reads `.wslconfig` as WSL does: `[section]` headers, `key = value` lines,
/// `#` comments, keys in any case, values maybe in double quotes.
pub fn parse_wslconfig(path: &str, text: Option<&str>) -> WslConfig {
    let mut config = WslConfig {
        path: path.to_string(),
        exists: text.is_some(),
        ..WslConfig::default()
    };
    let mut section = String::new();
    for line in text.unwrap_or("").lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(header) = line.strip_prefix('[') {
            section = header
                .split(']')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            config.has_wsl2 |= section == "wsl2";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if section != "wsl2" {
            continue;
        }
        let value = value.trim().trim_matches('"');
        match key.trim().to_ascii_lowercase().as_str() {
            "nestedvirtualization" => {
                config.nested = match value.to_ascii_lowercase().as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                }
            }
            "networkingmode" => config.networking = Some(value.to_string()),
            _ => {}
        }
    }
    config
}

/// `%UserProfile%\.wslconfig`, read if it is there.
pub fn read_wslconfig() -> WslConfig {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let path = std::path::Path::new(&home).join(".wslconfig");
    let text = std::fs::read_to_string(&path).ok();
    parse_wslconfig(&path.display().to_string(), text.as_deref())
}

/// The steps that set a `[wsl2]` key in `.wslconfig` and restart WSL.
pub fn wslconfig_steps(config: &WslConfig, line: &str) -> Vec<Step> {
    let path = &config.path;
    let place = if config.has_wsl2 {
        format!("In {path}, under the line [wsl2], put the line {line}")
    } else {
        format!("In {path} (new if missing), add a line [wsl2], and under it the line {line}")
    };
    vec![
        step(format!("{place}:"), format!("notepad \"{path}\"")),
        step("Then restart WSL so it reads the file:", "wsl --shutdown"),
    ]
}

/// Where a Linux system's items are fixed from: inside WSL from Windows, or a
/// shell on the Linux host itself.
#[derive(Clone, Copy)]
pub struct Place<'a> {
    pub distro: Option<&'a str>,
    pub config: &'a WslConfig,
}

impl Place<'_> {
    /// A command run as root in the distro, or with sudo on the host.
    pub fn root(&self, command: &str) -> String {
        match self.distro {
            Some(distro) => format!("wsl -d {distro} -u root -- {command}"),
            None => format!("sudo {command}"),
        }
    }

    pub fn install(&self, packages: &str) -> Vec<Step> {
        vec![
            run(self.root("apt-get update")),
            run(self.root(&format!("apt-get install -y {packages}"))),
        ]
    }

    /// A fresh start, after a change to WSL or to a group.
    pub fn restart(&self) -> Step {
        match self.distro {
            Some(_) => step("Restart WSL:", "wsl --shutdown"),
            None => say("Log out and back in."),
        }
    }

    /// The fix for a missing svm/vmx flag.
    pub fn virt_fix(&self) -> Vec<Step> {
        match (self.distro, self.config.nested) {
            (None, _) => vec![firmware()],
            (Some(_), Some(true)) => vec![
                step("WSL's own update may bring it:", "wsl --update"),
                run("wsl --shutdown"),
            ],
            (Some(_), _) => wslconfig_steps(self.config, "nestedVirtualization=true"),
        }
    }

    /// The fix for a missing /dev/kvm while the CPU flag is there.
    pub fn kvm_fix(&self) -> Vec<Step> {
        match self.distro {
            Some(_) => vec![
                step("Update WSL's kernel:", "wsl --update"),
                run("wsl --shutdown"),
            ],
            None => vec![step(
                "Load KVM (kvm_intel instead on an Intel CPU):",
                "sudo modprobe kvm_amd",
            )],
        }
    }

    /// The fix for QEMU without a display to draw on.
    pub fn display_fix(&self, r: &Report) -> Vec<Step> {
        match (r.wslg, self.distro) {
            (false, Some(_)) => {
                let mut steps = wslconfig_steps(self.config, "guiApplications=true");
                steps.insert(0, step("WSLg may be missing:", "wsl --update"));
                steps
            }
            _ => vec![say(
                "Run from a graphical session (DISPLAY or WAYLAND_DISPLAY set).",
            )],
        }
    }
}

/// Virtualization switched on in the PC's firmware.
pub fn firmware() -> Step {
    say(
        "In the PC's firmware (BIOS) setup, turn on SVM (AMD) or VT-x (Intel); it is often \
         under Advanced → CPU Configuration.",
    )
}

/// What a boot needs from the Linux side besides KVM, QEMU and OVMF.
#[derive(Clone, Copy)]
pub struct Needs {
    /// `run`: a QEMU window to type in.
    pub window: bool,
    /// `test` and `run --edit`: the guest reaches xtask's servers on Windows.
    pub loopback: bool,
}

pub fn nested(config: &WslConfig, virt_flags: u64) -> Item {
    let name = "nestedVirtualization";
    let edit = wslconfig_steps(config, "nestedVirtualization=true");
    match config.nested {
        Some(true) => pass(name, format!("true in {}", config.path)),
        Some(false) => fail(
            name,
            format!("false in {}: WSL gets no KVM", config.path),
            edit,
        ),
        None if virt_flags > 0 => pass(name, "not set; the default (true) is in effect"),
        None => fail(
            name,
            "not set; Windows 11's default should be true, yet WSL shows no svm flag",
            edit,
        ),
    }
}

/// The Linux side: virtualization flag, /dev/kvm, QEMU, its window, OVMF, and
/// the way back to Windows' localhost.
pub fn linux(r: &Report, place: Place, needs: Needs) -> Vec<Item> {
    let mut items = vec![virt_flag(r, place), kvm(r, place)];
    let version = r.qemu_version.as_deref().and_then(qemu_version);
    items.push(match &r.qemu {
        Some(path) => pass(
            "QEMU in WSL",
            format!("{} at {path}", version.unwrap_or_else(|| "found".into())),
        ),
        None => fail("QEMU in WSL", "not installed", place.install(PACKAGES)),
    });
    if needs.window {
        items.push(window(r, place));
    }
    items.push(match &r.ovmf {
        Some((code, _)) => pass("OVMF in WSL", code.clone()),
        None if r.qemu.is_none() => fail(
            "OVMF in WSL",
            "not installed",
            vec![say("The QEMU commands above install it too.")],
        ),
        None => fail("OVMF in WSL", "not installed", place.install("ovmf")),
    });
    if needs.loopback && place.distro.is_some() {
        items.push(loopback(r, place));
    }
    if place.distro.is_none() {
        for item in &mut items {
            item.name = item.name.replace(" in WSL", "");
        }
    }
    items
}

fn virt_flag(r: &Report, place: Place) -> Item {
    let name = "svm/vmx flag";
    if r.virt_flags > 0 {
        let found = format!("on {} CPUs: the system may run VMs", r.virt_flags);
        return pass(name, found);
    }
    let found = match place.distro {
        Some(_) => "none: WSL's VM gets no nested virtualization, so it has no /dev/kvm",
        None => "none: virtualization is off in the firmware or hidden by a hypervisor",
    };
    let fix = match (place.distro, place.config.nested) {
        // The nestedVirtualization item, shown just above, carries the edit.
        (Some(_), None | Some(false)) => vec![say(
            "Turn nestedVirtualization on (above), then run the same cargo xtask command again.",
        )],
        (Some(_), Some(true)) => {
            let mut fix = place.virt_fix();
            fix.push(say(
                "If the flag is still missing, use QEMU for Windows with WHPX.",
            ));
            fix
        }
        (None, _) => place.virt_fix(),
    };
    fail(name, found, fix)
}

fn kvm(r: &Report, place: Place) -> Item {
    let name = "/dev/kvm in WSL";
    match r.kvm {
        Kvm::Usable => pass(name, format!("{} may open it", r.user)),
        Kvm::Denied => fail(
            name,
            format!("exists, but {} may not open it", r.user),
            vec![
                run(place.root(&format!("usermod -aG kvm {}", r.user))),
                place.restart(),
            ],
        ),
        Kvm::Missing if r.virt_flags == 0 => fail(
            name,
            "missing, since there is no svm/vmx flag",
            vec![say("Fix the svm/vmx flag first.")],
        ),
        Kvm::Missing => fail(
            name,
            "missing, though the CPU flag is there",
            place.kvm_fix(),
        ),
    }
}

fn window(r: &Report, place: Place) -> Item {
    let name = "QEMU window (WSLg)";
    if r.qemu.is_none() {
        return fail(name, "no QEMU yet", vec![say("Install QEMU (above).")]);
    }
    if r.window() {
        return pass(name, "QEMU can open a window on the Windows desktop");
    }
    if !r.displays.iter().any(|d| d == "gtk" || d == "sdl") {
        let install = place.install("qemu-system-gui");
        return fail(name, "QEMU has no GTK or SDL display", install);
    }
    fail(
        name,
        "no display to draw on (WSLg is off)",
        place.display_fix(r),
    )
}

fn loopback(r: &Report, place: Place) -> Item {
    let name = "WSL → Windows localhost";
    if r.loopback == Some(true) {
        return pass(
            name,
            "reaches Windows' 127.0.0.1 (test and run --edit need it)",
        );
    }
    let found = "cannot reach Windows' 127.0.0.1, where test's mock LLM server listens";
    let mirrored = place
        .config
        .networking
        .as_deref()
        .is_some_and(|mode| mode.eq_ignore_ascii_case("mirrored"));
    let fix = if mirrored {
        vec![
            step(
                "networkingMode is already mirrored; update WSL:",
                "wsl --update",
            ),
            run("wsl --shutdown"),
        ]
    } else {
        wslconfig_steps(place.config, "networkingMode=mirrored")
    };
    fail(name, found, fix)
}

/// `QEMU 8.2.2` from `QEMU emulator version 8.2.2 (Debian 1:8.2.2+ds-0ubuntu1)`.
pub fn qemu_version(line: &str) -> Option<String> {
    let rest = line.split("version ").nth(1)?;
    let version = rest.split_whitespace().next()?;
    Some(format!("QEMU {version}"))
}
