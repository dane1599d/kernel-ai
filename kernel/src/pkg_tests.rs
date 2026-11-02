//! `test=pkg-store [undo|check]` (P2 task 13, the first slice of P4 task 1;
//! ADR 0011 §5, §12): the `pkg` service runs kaipkg's store on kaifs through
//! `fs`, holding only the `Dir` for `/system/pkg` its plan line's `dir=pkg`
//! grants. Three boots on one data disk:
//! - boot 1, `step=install`: generation 1, then `hello` installed as 2;
//! - boot 2, `undo`: 2 is still current; undo switches to 3, 1's packages;
//! - boot 3, `check`: 3 is current.
//!
//! The kernel's half waits for `pkg`'s `case/<step>.done` marker, which `pkg`
//! leaves only once its own line is printed, so the case never ends the boot
//! while the console still prints it. Then it reads
//! `current` through `fs` itself, by the `SYSTEM_PKG` badge, and checks it names
//! the generation that step leaves. On boot 1 it also checks that `pkg` holds
//! no capability bits and starts with that one `Dir`, and no home folder.
//!
//! `test=pkg-image [again|tampered]` (P4 task 1): `pkg`'s plan line adds
//! `bootfiles=pkgrepo`, and it makes the image's signed repository generation 1:
//! - boot 1, `step=image`: generation 1 holds the six apps, the five windows
//!   and the editor, from the image;
//! - boot 2, `again`: generation 1 is still current, seeded once;
//! - `tampered`, its own case on a fresh disk, with one byte of notes's target
//!   flipped in the bundle: notes is refused by name, and generation 1 holds
//!   the other five.
//!
//! Its kernel half reads `gen/1` through `fs` as well, and checks that `pkg`
//! holds the plan's `bootfiles=pkgrepo`, and that `pkg` added no package
//! version of any app: no plan line here runs one from a package, so `pkg`'s
//! `may-run` finds none to load (in v0.1 every built-in app runs the image's
//! own version).

use crate::process;
use crate::{handle, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use fsproto::{How, Reply, Request};
use kabi::handle::{Handle, Rights};

const SERVICE: &str = "fs";
const CLIENT: &str = "pkg";
/// `pkg` again in `pkg-install`, from a plan line without `dir=pkg`.
const PROBE: &str = "pkg-probe";
const CALL_MS: u64 = 10_000;
const WAIT_MS: u64 = 180_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Install,
    Undo,
    Check,
    Image,
    ImageAgain,
    ImageTampered,
    Mirror,
    MirrorAgain,
    MirrorTampered,
    MirrorRollback,
}

impl Step {
    /// The word after `test=pkg-store`: none for boot 1. `selftest::from_cmdline`
    /// runs at the top of `kmain`, before the heap exists, so this allocates
    /// nothing: a `Vec` here faulted before the banner and QEMU reset.
    pub fn from_cmdline(cmdline: &str) -> Self {
        let has = |word| cmdline.split_whitespace().any(|w| w == word);
        if has("check") {
            Step::Check
        } else if has("undo") {
            Step::Undo
        } else {
            Step::Install
        }
    }

    /// The word after `test=pkg-image`: none for boot 1. Before the heap, as
    /// [`Step::from_cmdline`], so it allocates nothing.
    pub fn image_from_cmdline(cmdline: &str) -> Self {
        let has = |word| cmdline.split_whitespace().any(|w| w == word);
        if has("tampered") {
            Step::ImageTampered
        } else if has("again") {
            Step::ImageAgain
        } else {
            Step::Image
        }
    }

    /// The word after `test=pkg-install`: none for boot 1. Before the heap, as
    /// [`Step::from_cmdline`], so it allocates nothing.
    pub fn mirror_from_cmdline(cmdline: &str) -> Self {
        let has = |word| cmdline.split_whitespace().any(|w| w == word);
        if has("tampered") {
            Step::MirrorTampered
        } else if has("rollback") {
            Step::MirrorRollback
        } else if has("again") {
            Step::MirrorAgain
        } else {
            Step::Mirror
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Step::Install => "install",
            Step::Undo => "undo",
            Step::Check => "check",
            Step::Image => "image",
            Step::ImageAgain => "image-again",
            Step::ImageTampered => "image-tampered",
            Step::Mirror => "mirror",
            Step::MirrorAgain => "mirror-again",
            Step::MirrorTampered => "mirror-tampered",
            Step::MirrorRollback => "mirror-rollback",
        }
    }

    fn case(self) -> &'static str {
        match self {
            Step::Install | Step::Undo | Step::Check => "pkg-store",
            Step::Image | Step::ImageAgain | Step::ImageTampered => "pkg-image",
            Step::Mirror | Step::MirrorAgain | Step::MirrorTampered | Step::MirrorRollback => {
                "pkg-install"
            }
        }
    }

    /// The boot it is, and the generation `current` names after it.
    fn boot_and_generation(self) -> (u32, u64) {
        match self {
            Step::Install => (1, 2),
            Step::Undo => (2, 3),
            Step::Check => (3, 3),
            Step::Image | Step::ImageTampered => (1, 1),
            Step::ImageAgain => (2, 1),
            // An install from the mirror; then undo and the install again.
            Step::Mirror => (1, 2),
            Step::MirrorAgain => (2, 4),
            // A tampered mirror switches nothing: generation 1 stays.
            Step::MirrorTampered => (1, 1),
            // Boot 2 of the older-snapshot case: boot 1 installed from the newer.
            Step::MirrorRollback => (2, 2),
        }
    }
}

/// Plan lines: `fs`, then `pkg` with `/system/pkg`. For `pkg-image`, `pkg` also
/// reads the image's repository and has a route to the supervisor. `pkg` needs a
/// deeper stack than the 16-page default: fsproto's minicbor decoder recurses,
/// and overflowed it in `Decoder::array`. `pkg-install` runs `pkg` again as
/// [`PROBE`], from a line without `dir=pkg`.
pub fn plan(step: Step) -> String {
    let fs = kabi::plan::FS;
    let step_name = step.name();
    match step.case() {
        "pkg-image" | "pkg-install" => format!(
            "{fs}\n{CLIENT} dir=pkg bootfiles=pkgrepo caps=call+serve uses=supervisor pages=2048 stack_pages=128 step={step_name}{}",
            match step.case() {
                "pkg-install" => format!("\n{PROBE} program={CLIENT} caps=serve step=not-holder"),
                _ => String::new(),
            }
        ),
        _ => format!("{fs}\n{CLIENT} dir=pkg pages=2048 stack_pages=128 step={step_name}"),
    }
}

pub fn check(step: Step) -> Result<(), &'static str> {
    let (boot, generation) = step.boot_and_generation();
    let case = step.case();
    wait_for(&format!("case/{}.done", step.name()))
        .map_err(|_| "pkg: pkg did not finish its step")?;
    let current = read("current")?;
    let current = core::str::from_utf8(&current).map_err(|_| "pkg: current is not text")?;
    if current.trim_end().parse::<u64>() != Ok(generation) {
        log!("{case}: boot {boot}: current holds {current:?}, wanted {generation}");
        return Err("pkg: current names another generation");
    }
    match step {
        Step::Install => only_its_folder()?,
        Step::Image | Step::ImageAgain | Step::ImageTampered => image_generation(step)?,
        Step::Mirror => mirror_installed()?,
        Step::Undo | Step::Check => {}
        Step::MirrorAgain | Step::MirrorTampered | Step::MirrorRollback => {}
    }
    log!("{case}: boot {boot}: fs shows generation {generation} current, as pkg left it");
    Ok(())
}

/// The image's packages, the window apps and the editor, each at 1.0.0, and the
/// service each runs as.
const WINDOWS: [(&str, &str); 6] = [
    ("editor", "editor"),
    ("files", "files-window"),
    ("monitor", "monitor-window"),
    ("notes", "notes-window"),
    ("settings", "settings-window"),
    ("terminal", "terminal-window"),
];

/// Generation 1 is the image's, its six apps, without notes when notes's
/// target was tampered with; and `pkg` holds the plan's `bootfiles=pkgrepo`.
fn image_generation(step: Step) -> Result<(), &'static str> {
    let first = read("gen/1")?;
    let first = core::str::from_utf8(&first).map_err(|_| "pkg-image: gen/1 is not text")?;
    let packages: Vec<&str> = first.lines().filter(|l| l.starts_with("pkg ")).collect();
    let mut held: Vec<&str> = packages
        .iter()
        .filter_map(|line| {
            let mut words = line.split(' ').skip(1);
            let (name, version) = (words.next()?, words.next()?);
            (version == "1.0.0").then_some(name)
        })
        .collect();
    held.sort_unstable();
    let tampered = matches!(step, Step::ImageTampered);
    let windows = WINDOWS.iter().map(|(name, _)| *name);
    let image: Vec<&str> = windows.filter(|n| !tampered || *n != "notes").collect();
    if held != image || held.len() != packages.len() {
        log!("pkg-image: gen/1 holds {packages:?}");
        return Err("pkg-image: generation 1 is not the image's packages");
    }
    let pid = process::named(CLIENT).first().map(|&(pid, _)| pid);
    let boot = process::with_table(|t| {
        t.iter()
            .find(|p| p.live() && Some(p.pid) == pid)
            .and_then(|p| p.boot)
    });
    if boot != Some(supervise::BootFiles::Pkgrepo) {
        return Err("pkg-image: pkg does not hold bootfiles=pkgrepo");
    }
    match step {
        Step::ImageTampered => log!(
            "pkg-image: generation 1 holds the five apps but notes: the tampered notes is not in it"
        ),
        _ => log!(
            "pkg-image: generation 1 is the six apps at 1.0.0, from the image's signed repository"
        ),
    }
    no_window_loaded()
}

/// `pkg` added no package version of any window: no plan line here runs one
/// from a package, so its `may-run` said so before any COMP_ADD.
fn no_window_loaded() -> Result<(), &'static str> {
    for (name, service) in WINDOWS {
        let added = process::components::component(service)
            .is_some_and(|c| c.labels().any(kabi::live::is_package_label));
        if added {
            log!("pkg-image: {name}'s window {service} has a package version");
            return Err("pkg-image: pkg loaded a window no plan line runs from a package");
        }
    }
    log!("pkg-image: pkg loaded no window: no plan line here runs one from a package");
    Ok(())
}

/// `pkg` holds no capability bits, and its startup table is the one `Dir` its
/// plan line grants: `/system/pkg`, not the home folder.
fn only_its_folder() -> Result<(), &'static str> {
    let pid = process::named(CLIENT).first().map(|&(pid, _)| pid);
    let slot = pid
        .and_then(|pid| process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid)))
        .ok_or("pkg-store: pkg has no slot")?;
    let (caps, folders) = process::with_table(|t| (t[slot].limits.caps, t[slot].folders));
    if caps != 0 || !folders {
        return Err("pkg-store: pkg holds capability bits, or no plan folder");
    }
    let badges: Vec<u64> = handle::with(slot, |h| {
        h.startup()
            .filter_map(|info| h.table.get(Handle(info.handle)).ok().map(|e| e.badge))
            .collect()
    })
    .map_err(|_| "pkg-store: pkg has no table")?;
    if badges != [kabi::fs::SYSTEM_PKG] {
        log!("pkg-store: pkg's startup badges are {badges:x?}");
        return Err("pkg-store: pkg's startup Dirs are not just /system/pkg");
    }
    log!("pkg-store: pkg holds no capability bits and starts with only /system/pkg's Dir");
    Ok(())
}

/// Generation 2 holds `big`, from the mirror, beside the image's notes.
fn mirror_installed() -> Result<(), &'static str> {
    let second = read("gen/2")?;
    let second = core::str::from_utf8(&second).map_err(|_| "pkg-install: gen/2 is not text")?;
    let named = |name: &str| {
        second
            .lines()
            .any(|l| l.starts_with(&format!("pkg {name} ")))
    };
    if !named("big") || !named("notes") {
        log!("pkg-install: gen/2 is {second:?}");
        return Err("pkg-install: generation 2 is not notes and big");
    }
    log!("pkg-install: generation 2 holds big from the mirror beside the image's notes");
    catalogue_lists_them()?;
    only_the_holder_calls()
}

/// `name`'s answer to `request`, asked again until it serves, or None.
fn answer_of(name: &str, request: &[u8]) -> Option<Vec<u8>> {
    for _ in 0..WAIT_MS / 50 {
        if let Ok(reply) = ipc::call(name, request, CALL_MS) {
            return Some(reply);
        }
        threads::sleep_ms(50);
    }
    None
}

/// [`PROBE`], `pkg` from a plan line without `dir=pkg`, got Permission from
/// each call of lane P's block, PKG_FETCH among them with a request the
/// holder's mirror would answer.
fn only_the_holder_calls() -> Result<(), &'static str> {
    let answer = answer_of(PROBE, b"result").ok_or("pkg-install: pkg-probe never answered")?;
    if answer != b"ok" {
        log!(
            "pkg-install: pkg-probe says {:?}",
            core::str::from_utf8(&answer)
        );
        return Err("pkg-install: a process without /system/pkg was not refused");
    }
    log!(
        "pkg-install: a process without /system/pkg gets Permission from BOOT_READ, COMP_ADD, PKG_FETCH and PKG_TIME"
    );
    Ok(())
}

/// `pkg` answers the Apps sheet's `catalog` (P4 task 3): big and the image's
/// six apps as installed, then the mirror's `extra`, which is not.
fn catalogue_lists_them() -> Result<(), &'static str> {
    let answer =
        answer_of(CLIENT, b"catalog 0").ok_or("pkg-install: pkg never answered catalog")?;
    let text =
        core::str::from_utf8(&answer).map_err(|_| "pkg-install: the catalogue is not text")?;
    let rows: Vec<&str> = text.lines().collect();
    let wanted = [
        "kai-catalog 1",
        "installed\tbig\t1.0.0\tdata\t",
        "installed\teditor\t1.0.0\tapp\t",
        "installed\tfiles\t1.0.0\tapp\t",
        "installed\tmonitor\t1.0.0\tapp\t",
        "installed\tnotes\t1.0.0\tapp\t",
        "installed\tsettings\t1.0.0\tapp\t",
        "installed\tterminal\t1.0.0\tapp\t",
        "mirror\textra\t1.0.0\tdata\t",
    ];
    let listed = rows.len() == wanted.len()
        && rows
            .iter()
            .zip(wanted)
            .all(|(row, start)| row.starts_with(start));
    if !listed {
        log!("pkg-install: the catalogue is {text:?}");
        return Err("pkg-install: the catalogue does not list installed then offered");
    }
    log!(
        "pkg-install: the catalogue lists big and the six apps installed, then the mirror's extra"
    );
    Ok(())
}

/// One request to `fs` on `badge`, as its kernel client.
fn ask(badge: u64, request: Request) -> Result<Reply, &'static str> {
    let bytes = fsproto::encode_request(&request);
    let deadline = crate::hpet::nanos() + CALL_MS * 1_000_000;
    let extra = ipc::Extra {
        badge,
        rights: (Rights::READ | Rights::ENUMERATE).bits(),
        handles: Vec::new(),
    };
    let (reply, _, _) = ipc::call_with(SERVICE, &bytes, 0, deadline, || false, extra)
        .map_err(|_| "pkg: a call to fs failed")?;
    fsproto::decode_reply(&reply).map_err(|_| "pkg: fs's reply does not decode")
}

/// `/system/pkg/<path>`, whole.
fn read(path: &str) -> Result<Vec<u8>, &'static str> {
    let opened = ask(
        kabi::fs::SYSTEM_PKG,
        Request::Open {
            path: path.into(),
            rights: Rights::READ.bits(),
            how: How::Existing,
        },
    )?;
    let Reply::Opened(stat, file) = opened else {
        return Err("pkg: a file in /system/pkg cannot be opened");
    };
    let mut out = Vec::new();
    while (out.len() as u64) < stat.size {
        let request = Request::Read {
            off: out.len() as u64,
            len: fsproto::MAX_DATA as u32,
        };
        match ask(file, request)? {
            Reply::Data(data) if !data.is_empty() => out.extend_from_slice(&data),
            _ => return Err("pkg: a read in /system/pkg failed"),
        }
    }
    Ok(out)
}

/// Waits until `/system/pkg/<path>` exists.
pub(crate) fn wait_for(path: &str) -> Result<(), ()> {
    for _ in 0..WAIT_MS / 50 {
        let open = Request::Open {
            path: path.into(),
            rights: 0,
            how: How::Existing,
        };
        if ipc::server_of(SERVICE).is_some()
            && matches!(ask(kabi::fs::SYSTEM_PKG, open), Ok(Reply::Opened(..)))
        {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    Err(())
}
