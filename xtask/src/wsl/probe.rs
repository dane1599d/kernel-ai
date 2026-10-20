//! What a Linux system offers for running Kernel AI, found by one shell script:
//! /dev/kvm, the CPU's virtualization flag, QEMU and its window, OVMF, memory, and
//! (inside WSL) whether the distro reaches Windows' 127.0.0.1. `--accel wsl`
//! refuses to start without what it needs. The script
//! runs in a WSL2 distro through `wsl.exe -e`, or natively on a Linux host. It only
//! reads: no package, setting or file is changed.

use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// The OVMF images a Linux distribution installs, code then variables, in the
/// order xtask looks for them, natively and inside WSL.
pub const LINUX_OVMF: &[(&str, &str)] = &[
    (
        "/usr/share/OVMF/OVMF_CODE_4M.fd",
        "/usr/share/OVMF/OVMF_VARS_4M.fd",
    ),
    (
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/OVMF/OVMF_VARS.fd",
    ),
    (
        "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
        "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
    ),
    (
        "/usr/share/edk2/ovmf/OVMF_CODE.fd",
        "/usr/share/edk2/ovmf/OVMF_VARS.fd",
    ),
];

/// The first `wsl.exe` call after WSL stopped boots its VM: 90 s on the dev PC.
pub const START_LIMIT: Duration = Duration::from_secs(240);
/// Any other probe: `wsl -l`, PowerShell queries, `qemu --version`.
pub const SHORT_LIMIT: Duration = Duration::from_secs(60);
/// The distro xtask uses when it is set; otherwise WSL's default distro.
pub const DISTRO_ENV: &str = "KAI_WSL_DISTRO";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kvm {
    /// Exists and opens for reading and writing.
    Usable,
    /// Exists, but the user may not open it.
    Denied,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub kvm: Kvm,
    /// Lines of /proc/cpuinfo with `svm` (AMD) or `vmx` (Intel): one per CPU when
    /// the system may run its own VMs, 0 when its hypervisor hides the feature.
    pub virt_flags: u64,
    pub cpus: Option<u64>,
    pub mem_kib: Option<u64>,
    pub user: String,
    pub qemu: Option<String>,
    pub qemu_version: Option<String>,
    /// Display back ends QEMU lists (`gtk`, `sdl`, `none`, ...).
    pub displays: Vec<String>,
    /// OVMF code and variable store, the first pair of `LINUX_OVMF` present.
    pub ovmf: Option<(String, String)>,
    /// WSLg's mount and a display to draw on, so QEMU's window reaches Windows.
    pub wslg: bool,
    pub has_display: bool,
    /// Where Windows drives are mounted, `/mnt/` unless /etc/wsl.conf moves them.
    pub mount_root: String,
    /// Whether a connection to 127.0.0.1 reached a listener on Windows: `test`'s
    /// mock LLM server and `run --edit`'s build service are there.
    pub loopback: Option<bool>,
}

impl Report {
    /// QEMU can open a window: a GTK or SDL display and WSLg (or X) to show it.
    pub fn window(&self) -> bool {
        let backend = self.displays.iter().any(|d| d == "gtk" || d == "sdl");
        backend && self.has_display
    }
}

/// The probe, a POSIX shell script printing `key=value` lines. Its first
/// argument is a Windows loopback port to connect to, or 0.
pub fn script() -> String {
    let ovmf: Vec<String> = LINUX_OVMF
        .iter()
        .map(|(code, vars)| format!("\"{code}|{vars}\""))
        .collect();
    let lines = [
        "kvm=missing",
        "if [ -e /dev/kvm ]; then if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then kvm=usable; else kvm=denied; fi; fi",
        "echo \"kvm=$kvm\"",
        "echo \"virt_flags=$(grep -E '^flags' /proc/cpuinfo | grep -c -w -E 'svm|vmx')\"",
        "echo \"cpus=$(nproc)\"",
        "echo \"mem_kib=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)\"",
        "echo \"user=$(id -un)\"",
        "q=$(command -v qemu-system-x86_64)",
        "echo \"qemu=$q\"",
        "if [ -n \"$q\" ]; then echo \"qemu_version=$(\"$q\" --version | head -n 1)\"; echo \"displays=$(\"$q\" -display help 2>/dev/null | tr '\\n' ' ')\"; fi",
        &format!(
            "for p in {}; do c=${{p%|*}}; v=${{p#*|}}; if [ -f \"$c\" ] && [ -f \"$v\" ]; then echo \"ovmf=$p\"; break; fi; done",
            ovmf.join(" ")
        ),
        "if [ -d /mnt/wslg ]; then echo wslg=yes; else echo wslg=no; fi",
        "if [ -n \"$DISPLAY$WAYLAND_DISPLAY\" ]; then echo display=yes; else echo display=no; fi",
        "echo \"c_drive=$(wslpath -u 'C:\\' 2>/dev/null)\"",
        "if [ \"$1\" != 0 ]; then if timeout 5 bash -c \"exec 3<>/dev/tcp/127.0.0.1/$1\" 2>/dev/null; then echo loopback=yes; else echo loopback=no; fi; fi",
        "echo done=yes",
    ];
    lines.join("\n")
}

/// Reads the script's output. A report without its last line is refused, so a
/// probe cut short is never taken for a system without QEMU.
pub fn parse_report(text: &str) -> Result<Report, String> {
    let mut report = Report {
        kvm: Kvm::Missing,
        virt_flags: 0,
        cpus: None,
        mem_kib: None,
        user: String::new(),
        qemu: None,
        qemu_version: None,
        displays: Vec::new(),
        ovmf: None,
        wslg: false,
        has_display: false,
        mount_root: "/mnt/".into(),
        loopback: None,
    };
    let mut done = false;
    let some = |v: &str| (!v.is_empty()).then(|| v.to_string());
    for line in text.lines() {
        let Some((key, value)) = line.trim_end_matches('\r').split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "kvm" => {
                report.kvm = match value {
                    "usable" => Kvm::Usable,
                    "denied" => Kvm::Denied,
                    _ => Kvm::Missing,
                }
            }
            "virt_flags" => report.virt_flags = value.parse().unwrap_or(0),
            "cpus" => report.cpus = value.parse().ok(),
            "mem_kib" => report.mem_kib = value.parse().ok(),
            "user" => report.user = value.to_string(),
            "qemu" => report.qemu = some(value),
            "qemu_version" => report.qemu_version = some(value),
            "displays" => report.displays = value.split_whitespace().map(String::from).collect(),
            "ovmf" => {
                report.ovmf = value
                    .split_once('|')
                    .map(|(code, vars)| (code.to_string(), vars.to_string()))
            }
            "wslg" => report.wslg = value == "yes",
            "display" => report.has_display = value == "yes",
            "c_drive" => {
                if let Some(root) = mount_root(value) {
                    report.mount_root = root;
                }
            }
            "loopback" => report.loopback = Some(value == "yes"),
            "done" => done = true,
            _ => {}
        }
    }
    if done {
        Ok(report)
    } else {
        Err("the probe stopped before its end".into())
    }
}

/// The mount root from `wslpath -u 'C:\'`: `/mnt/c/` gives `/mnt/`, `/c/` gives `/`.
pub fn mount_root(c_drive: &str) -> Option<String> {
    let root = c_drive.trim().trim_end_matches('/').strip_suffix('c')?;
    root.ends_with('/').then(|| root.to_string())
}

/// A WSL distro as `wsl -l -v` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distro {
    pub name: String,
    pub version: u32,
    pub default: bool,
}

/// Why there is no distro to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoDistro {
    /// WSL itself is not installed.
    NotInstalled,
    /// `wsl -l -v` failed or gave no answer.
    ListFailed(String),
    /// WSL lists no Linux distribution (Docker Desktop's do not count).
    NoneListed,
    /// `KAI_WSL_DISTRO` names this distro, which WSL does not list; it lists these.
    NotListed(String, Vec<String>),
}

impl NoDistro {
    pub fn describe(&self) -> String {
        match self {
            NoDistro::NotInstalled => "not installed".into(),
            NoDistro::ListFailed(e) => format!("wsl -l -v failed: {e}"),
            NoDistro::NoneListed => "installed, but with no Linux distribution".into(),
            NoDistro::NotListed(name, listed) => format!(
                "{DISTRO_ENV} names {name}, which WSL does not list (it lists {})",
                listed.join(", ")
            ),
        }
    }
}

/// Reads `wsl -l -v`: a header line, then `* NAME STATE VERSION` rows, the `*`
/// marking the default. The state may be several words in another language.
pub fn parse_list(text: &str) -> Vec<Distro> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (default, rest) = match line.strip_prefix('*') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            let words: Vec<&str> = rest.split_whitespace().collect();
            let [name, _, .., version] = words.as_slice() else {
                return None;
            };
            Some(Distro {
                name: name.to_string(),
                version: version.parse().ok()?,
                default,
            })
        })
        .collect()
}

/// The distro to use: the one named (by `KAI_WSL_DISTRO`), else the default,
/// else the first; never Docker Desktop's own distros, which have no packages.
pub fn choose<'d>(distros: &'d [Distro], wanted: Option<&str>) -> Result<&'d Distro, NoDistro> {
    if let Some(name) = wanted.filter(|n| !n.is_empty()) {
        return distros
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let listed = distros.iter().map(|d| d.name.clone()).collect();
                NoDistro::NotListed(name.to_string(), listed)
            });
    }
    let usable = |d: &&Distro| !d.name.starts_with("docker-desktop");
    distros
        .iter()
        .filter(usable)
        .find(|d| d.default)
        .or_else(|| distros.iter().find(usable))
        .ok_or(NoDistro::NoneListed)
}

/// Text from `wsl.exe`, which writes UTF-16 unless WSL_UTF8 is honoured.
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
    let utf16 = bytes.len() >= 2 && bytes.len().is_multiple_of(2) && bytes[1] == 0 && bytes[0] != 0;
    if utf16 {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// What a finished command printed.
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a command with nothing on stdin, and kills it if it outlives `limit`.
pub fn run(command: &mut Command, limit: Duration) -> Result<Output, String> {
    let name = format!("{:?}", command.get_program());
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {name}: {e}"))?;
    let stdout = drain(&mut child, true);
    let stderr = drain(&mut child, false);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} gave no answer in {} s", limit.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("waiting on {name}: {e}")),
        }
    };
    Ok(Output {
        success: status.success(),
        stdout: decode(&stdout.join().unwrap_or_default()),
        stderr: decode(&stderr.join().unwrap_or_default()),
    })
}

fn drain(child: &mut Child, stdout: bool) -> thread::JoinHandle<Vec<u8>> {
    let mut pipe: Option<Box<dyn Read + Send>> = if stdout {
        child.stdout.take().map(|p| Box::new(p) as _)
    } else {
        child.stderr.take().map(|p| Box::new(p) as _)
    };
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = pipe.as_mut() {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

/// WSL's own wsl.exe, found from its files and never by running one: on a PC
/// without WSL, System32's wsl.exe is a stub that offers to install WSL, with
/// elevation, at a key press. The package (WSL 2.x) lives in Program Files; the
/// Windows 10 built-in WSL is System32's wsl.exe with its service DLL beside it.
pub fn exe() -> Option<PathBuf> {
    let dir = |var: &str, default: &str| {
        std::env::var_os(var).map_or_else(|| PathBuf::from(default), PathBuf::from)
    };
    let package = dir("ProgramFiles", r"C:\Program Files").join(r"WSL\wsl.exe");
    let system = dir("SystemRoot", r"C:\Windows").join("System32");
    let inbox = system.join(r"lxss\LxssManager.dll");
    pick_exe(
        package.exists(),
        inbox.exists(),
        package,
        system.join("wsl.exe"),
    )
}

/// The wsl.exe to run, given which of WSL's files exist; never the bare stub.
pub fn pick_exe(
    package: bool,
    inbox: bool,
    package_exe: PathBuf,
    system_exe: PathBuf,
) -> Option<PathBuf> {
    match (package, inbox) {
        (true, _) => Some(package_exe),
        (false, true) => Some(system_exe),
        (false, false) => None,
    }
}

/// WSL's wsl.exe (see `exe`), asked to write UTF-8, in a hidden console of its
/// own, so no key typed in the user's terminal can reach a prompt of its.
pub fn wsl() -> Command {
    let mut command = Command::new(exe().unwrap_or_else(|| PathBuf::from("wsl.exe")));
    command.env("WSL_UTF8", "1");
    hide_console(&mut command);
    command
}

/// A console of the child's own, never shown: it cannot change the user's
/// terminal's modes, and no key typed there reaches it.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = command;
}

/// The distros WSL knows, from `wsl -l -v`.
pub fn list() -> Result<Vec<Distro>, NoDistro> {
    if exe().is_none() {
        return Err(NoDistro::NotInstalled);
    }
    let out = run(wsl().args(["-l", "-v"]), SHORT_LIMIT).map_err(NoDistro::ListFailed)?;
    let distros = parse_list(&out.stdout);
    if out.success || !distros.is_empty() {
        Ok(distros)
    } else {
        Err(NoDistro::ListFailed(first_line(&out.stdout, &out.stderr)))
    }
}

/// The distro xtask uses: `KAI_WSL_DISTRO`'s, else WSL's default.
pub fn find() -> Result<Distro, NoDistro> {
    let distros = list()?;
    let wanted = std::env::var(DISTRO_ENV).ok();
    choose(&distros, wanted.as_deref()).cloned()
}

/// Runs the probe in `distro`, starting it if needed, and checks that it reaches a
/// listener on Windows' 127.0.0.1. Returns the report and what WSL said on stderr.
pub fn ask(distro: &str) -> Result<(Report, Option<String>), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot open a loopback port to test: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let script = script();
    let out = run(
        wsl().args([
            "-d",
            distro,
            "-e",
            "sh",
            "-c",
            &script,
            "sh",
            &port.to_string(),
        ]),
        START_LIMIT,
    )?;
    let mut report = parse_report(&out.stdout)
        .map_err(|e| format!("{e}: {}", first_line(&out.stdout, &out.stderr)))?;
    // The connection counts only if it reached this listener, not one in WSL.
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let arrived = listener.accept().is_ok();
    report.loopback = report.loopback.map(|reached| reached && arrived);
    let note = out.stderr.trim();
    Ok((report, (!note.is_empty()).then(|| note.to_string())))
}

/// The first line of stderr, else of stdout: the useful part of a failure.
fn first_line(stdout: &str, stderr: &str) -> String {
    [stderr, stdout]
        .iter()
        .find_map(|text| text.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("no output")
        .to_string()
}
