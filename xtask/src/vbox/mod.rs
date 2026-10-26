//! `cargo xtask vbox [--model-disk PATH | --local ID [--store DIR] | --no-model]`
//! builds the release kernel and its boot image and creates, or updates, the
//! VirtualBox VM "Kernel AI" to boot it. `cargo xtask vbox --start [--headless]`
//! then boots that VM as it is. `--cmdline WORDS` adds kernel command-line words.
//! `--name NAME` works on another VM instead, in its own folder
//! (`D:\kai-vbox-<name>`), so a test VM leaves "Kernel AI" alone.
//!
//! The VM is a modern PC: EFI, the ICH9 chipset (PCIe, so MCFG), HPET, IOAPIC,
//! xAPIC, a USB keyboard and tablet on an xHCI controller (PS/2 stays as the
//! fallback until the kernel configures them), a virtio-net card on NAT, and
//! one NVMe controller with the OS disk (the image as a VDI) on port 0 and the
//! model on port 1. The model is read in place through a VMDK descriptor
//! (`vmdk.rs`), never copied. The vm(), its disks and the serial log live in `KAI_VBOX_DIR`
//! (`D:\kai-vbox` by default). The model runs inside the VM only.
//!
//! The tablet ends VirtualBox's mouse capture: the pointer moves in and out of
//! the window without Right Ctrl. Since VirtualBox 7.0.0 the xHCI controller is
//! in the base package, so it needs no Oracle Extension Pack.

pub mod doctor;
pub mod manage;
mod vmdk;

use crate::{components, disk, esp, floatguard, image, models, options, userbuild};
use manage::{CONTROLLER, step, vm};
use std::fs;
use std::path::{Path, PathBuf};

/// Guest memory: Qwen3-4B needs a 4,160 MiB guest; 6 GiB leaves room.
const MEMORY_MIB: u64 = 6144;
const OS_DISK: &str = "kai-os.vdi";
const MODEL_DISK: &str = "kai-model.vmdk";
const MODEL_TAIL: &str = "kai-model.tail";
const SERIAL_LOG: &str = "serial.log";
/// The kernel's clock and tick rate in the VM: HPET reads and APIC timer exits are
/// slow under VirtualBox on Hyper-V (kernel/src/hpet.rs and timer.rs).
const CLOCK: &str = "clock=tsc hz=250";

struct Args {
    /// `--name`: the VM to create or boot, "Kernel AI" by default.
    name: String,
    /// `--cmdline WORDS`: more kernel command-line words, such as `llm=HOST:PORT`.
    cmdline: String,
    start: bool,
    headless: bool,
    model: Model,
}

enum Model {
    Default,
    None,
    Path(PathBuf),
    Local { id: String, store: Option<String> },
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args {
        name: manage::DEFAULT_VM.into(),
        cmdline: String::new(),
        start: false,
        headless: false,
        model: Model::Default,
    };
    let mut store = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--start" => parsed.start = true,
            "--name" => parsed.name = value()?,
            "--cmdline" => parsed.cmdline = value()?,
            "--headless" => parsed.headless = true,
            "--no-model" => parsed.model = Model::None,
            "--model-disk" => parsed.model = Model::Path(PathBuf::from(value()?)),
            "--local" => {
                parsed.model = Model::Local {
                    id: value()?,
                    store: None,
                }
            }
            "--store" => store = Some(value()?),
            other => return Err(format!("vbox: unknown option {other}")),
        }
    }
    if let (Some(dir), Model::Local { store, .. }) = (store.clone(), &mut parsed.model) {
        *store = Some(dir);
    } else if let (Some(dir), Model::Default) = (store, &parsed.model) {
        parsed.model = Model::Local {
            id: String::new(),
            store: Some(dir),
        };
    }
    if parsed.headless && !parsed.start {
        return Err("vbox: --headless goes with --start".into());
    }
    Ok(parsed)
}

pub fn main(args: &[String]) -> Result<(), String> {
    let args = parse(args)?;
    manage::set_vm(&args.name);
    let dir = vm_dir(&args.name);
    if args.start {
        return start(args.headless, &dir);
    }
    let version = manage::version().ok_or("VBoxManage does not run: install VirtualBox 7")?;
    println!(
        "xtask: VirtualBox {version}; the VM lives in {}",
        dir.display()
    );
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let model = model_path(&args.model)?;
    let disk = model.as_deref().map(disk::ModelDisk::open).transpose()?;
    let model_words = disk
        .as_ref()
        .map_or_else(String::new, disk::ModelDisk::cmdline);
    let cmdline = format!("{CLOCK} {model_words} {}", args.cmdline);
    let esp = build_esp(&cmdline)?;
    let raw = dir.join("kai-os.img");
    let bytes = image::build(&esp, &raw)?;
    println!("xtask: boot image {} ({} MiB)", raw.display(), bytes >> 20);
    let memory = disk
        .as_ref()
        .map_or(MEMORY_MIB, |d| d.memory_mib().max(MEMORY_MIB));
    configure(&dir, &raw, model.as_deref(), memory)?;
    let _ = fs::remove_file(&raw);
    let name = match vm() {
        manage::DEFAULT_VM => String::new(),
        other => format!(" --name \"{other}\""),
    };
    println!(
        "xtask: the VM {:?} is ready; boot it with: cargo xtask vbox --start{name}",
        vm()
    );
    println!(
        "xtask: its serial console goes to {}",
        dir.join(SERIAL_LOG).display()
    );
    Ok(())
}

/// `KAI_VBOX_DIR`, or `D:\kai-vbox` for "Kernel AI" and `D:\kai-vbox-<name>` for
/// another VM: never C:, which is short of space.
fn vm_dir(name: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os("KAI_VBOX_DIR") {
        return PathBuf::from(dir);
    }
    match name {
        manage::DEFAULT_VM => PathBuf::from(r"D:\kai-vbox"),
        other => {
            let slug: String = other
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            PathBuf::from(format!(r"D:\kai-vbox-{slug}"))
        }
    }
}

fn model_path(model: &Model) -> Result<Option<PathBuf>, String> {
    match model {
        Model::None => Ok(None),
        Model::Path(path) => Ok(Some(path.clone())),
        Model::Default => models::stored(&models::default_id()?, None).map(Some),
        Model::Local { id, store } => {
            let id = if id.is_empty() {
                models::default_id()?
            } else {
                id.clone()
            };
            models::stored(&id, store.as_deref()).map(Some)
        }
    }
}

/// Builds the release kernel, the programs and the components, and lays out the
/// directory ESP the image is made from.
fn build_esp(cmdline: &str) -> Result<PathBuf, String> {
    let root = crate::workspace_root();
    let options = options::parse_args(&["build".into(), "--release".into()])?;
    let kernel = crate::build_kernel(&root, &options)?;
    floatguard::check(&kernel)?;
    let programs = userbuild::build_user(&root, &options)?;
    let target_dir = root.join("target");
    let mut modules = esp::boot_modules(&kernel, &programs, &root)?;
    modules.extend(components::build(&root)?);
    let efi = crate::ensure_limine(&target_dir)?;
    let esp = target_dir.join("vbox-esp");
    esp::make_esp(&esp, &kernel, &modules, &efi, cmdline)?;
    Ok(esp)
}

/// Creates the VM if needed, sets it up, and attaches fresh disks.
fn configure(dir: &Path, raw: &Path, model: Option<&Path>, memory: u64) -> Result<(), String> {
    let info = match manage::vm_info() {
        Some(info) => info,
        None => {
            let base = path_str(dir)?;
            step(&[
                "createvm",
                "--name",
                vm(),
                "--ostype",
                "Other_64",
                "--basefolder",
                base,
                "--register",
            ])?;
            manage::vm_info().ok_or("the new VM is not registered")?
        }
    };
    if let Some(state) = info
        .get("VMState")
        .filter(|s| *s != "poweroff" && *s != "aborted")
    {
        return Err(format!(
            "the VM is {state}: power it off first (VBoxManage controlvm \"{}\" poweroff)",
            vm()
        ));
    }
    let has_controller = info.values().any(|v| v == CONTROLLER);
    // Free the old disks so their files can be replaced.
    if has_controller {
        for port in ["0", "1"] {
            let _ = manage::run(&[
                "storageattach",
                vm(),
                "--storagectl",
                CONTROLLER,
                "--port",
                port,
                "--device",
                "0",
                "--medium",
                "none",
            ]);
        }
    }
    let os_disk = dir.join(OS_DISK);
    let model_disk = dir.join(MODEL_DISK);
    manage::forget(&os_disk, dir)?;
    if os_disk.exists() {
        fs::remove_file(&os_disk).map_err(|e| format!("cannot replace {OS_DISK}: {e}"))?;
    }
    step(&[
        "convertfromraw",
        path_str(raw)?,
        path_str(&os_disk)?,
        "--format",
        "VDI",
    ])?;
    modify(dir, memory)?;
    if !has_controller {
        step(&[
            "storagectl",
            vm(),
            "--name",
            CONTROLLER,
            "--add",
            "pcie",
            "--controller",
            "NVMe",
            "--portcount",
            "2",
            "--bootable",
            "on",
        ])?;
    }
    attach("0", &os_disk, "normal")?;
    if let Some(model) = model {
        let vmdk = vmdk::for_file(dir, model, MODEL_TAIL)?;
        // VirtualBox writes its UUIDs into the descriptor once it registers it; a
        // registered one naming the same extents is kept, registration and all.
        let old = fs::read_to_string(&model_disk).unwrap_or_default();
        let registered = old.contains("ddb.uuid.image");
        if registered && vmdk::extents(&old) == vmdk::extents(&vmdk.descriptor) {
            manage::forget_children(&model_disk, dir)?;
        } else {
            manage::forget(&model_disk, dir)?;
            if let Some(tail) = vmdk.tail {
                fs::write(dir.join(MODEL_TAIL), tail)
                    .map_err(|e| format!("cannot write {MODEL_TAIL}: {e}"))?;
            }
            fs::write(&model_disk, vmdk.descriptor)
                .map_err(|e| format!("cannot write {MODEL_DISK}: {e}"))?;
        }
        println!(
            "xtask: the model {} is port 1, read in place",
            model.display()
        );
        attach("1", &model_disk, "immutable")?;
    }
    Ok(())
}

fn attach(port: &str, medium: &Path, kind: &str) -> Result<(), String> {
    step(&[
        "storageattach",
        vm(),
        "--storagectl",
        CONTROLLER,
        "--port",
        port,
        "--device",
        "0",
        "--type",
        "hdd",
        "--mtype",
        kind,
        "--medium",
        path_str(medium)?,
    ])
    .map(|_| ())
}

fn modify(dir: &Path, memory: u64) -> Result<(), String> {
    let host = std::thread::available_parallelism().map_or(1, usize::from);
    let cpus = host.clamp(1, options::MAX_VM_CPUS).to_string();
    let memory = memory.to_string();
    let serial = dir.join(SERIAL_LOG);
    #[rustfmt::skip]
    let args = [
        "modifyvm", vm(),
        "--firmware", "efi", "--chipset", "ich9", "--hpet", "on", "--ioapic", "on",
        "--x2apic", "off", "--apic", "on", "--cpus", &cpus, "--memory", &memory, "--vram", "16",
        "--graphicscontroller", "vmsvga", "--mouse", "usbtablet", "--keyboard", "usb",
        "--nic1", "nat", "--nictype1", "virtio", "--nat-localhostreachable1", "on",
        "--uart1", "0x3F8", "4", "--uartmode1", "file", path_str(&serial)?,
        "--boot1", "disk", "--boot2", "none", "--boot3", "none", "--boot4", "none",
        "--audio-enabled", "off", "--usbohci", "off", "--usbehci", "off", "--usbxhci", "on",
        "--paravirtprovider", "minimal", "--rtcuseutc", "on",
    ];
    step(&args).map(|_| ())
}

fn start(headless: bool, dir: &Path) -> Result<(), String> {
    let info = manage::vm_info()
        .ok_or_else(|| format!("no VM {:?} yet: run cargo xtask vbox first", vm()))?;
    if info.get("VMState").is_some_and(|s| s == "running") {
        return Err("the VM is already running".into());
    }
    let kind = if headless { "headless" } else { "gui" };
    step(&["startvm", vm(), "--type", kind])?;
    println!(
        "xtask: booting; the serial console is in {}",
        dir.join(SERIAL_LOG).display()
    );
    Ok(())
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{} is not a UTF-8 path", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Result<Args, String> {
        parse(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn options_parse() {
        let plain = args(&[]).unwrap();
        assert!(!plain.start && matches!(plain.model, Model::Default));
        let start = args(&["--start", "--headless"]).unwrap();
        assert!(start.start && start.headless);
        assert!(args(&["--headless"]).is_err());
        assert!(matches!(args(&["--no-model"]).unwrap().model, Model::None));
        match args(&["--local", "qwen3-0.6b", "--store", "E:\\m"])
            .unwrap()
            .model
        {
            Model::Local { id, store } => {
                assert_eq!(id, "qwen3-0.6b");
                assert_eq!(store.as_deref(), Some("E:\\m"));
            }
            _ => panic!("not local"),
        }
        assert!(matches!(
            args(&["--store", "E:\\m"]).unwrap().model,
            Model::Local { ref id, .. } if id.is_empty()
        ));
        assert!(args(&["--model-disk"]).is_err());
        assert_eq!(
            args(&["--name", "Kernel AI test"]).unwrap().name,
            "Kernel AI test"
        );
        assert_eq!(args(&[]).unwrap().name, "Kernel AI");
        assert_eq!(
            args(&["--cmdline", "llm=10.0.2.2:8080"]).unwrap().cmdline,
            "llm=10.0.2.2:8080"
        );
        if std::env::var_os("KAI_VBOX_DIR").is_none() {
            assert_eq!(vm_dir("Kernel AI"), PathBuf::from(r"D:\kai-vbox"));
            assert_eq!(
                vm_dir("Kernel AI test"),
                PathBuf::from(r"D:\kai-vbox-kernel-ai-test")
            );
        }
        assert!(args(&["--bogus"]).is_err());
    }
}
