//! VBoxManage, run for the one VM xtask works on: "Kernel AI", or the one
//! `--name` names. Nothing here changes VirtualBox's global settings or any other
//! VM.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_VM: &str = kabi::PRODUCT_NAME;
static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The VM this run works on.
pub fn vm() -> &'static str {
    NAME.get().map_or(DEFAULT_VM, String::as_str)
}

/// Picks the VM for this run (`--name`); once, before any VBoxManage call.
pub fn set_vm(name: &str) {
    let _ = NAME.set(name.to_string());
}
/// The storage controller xtask adds: NVMe, as on a modern PC.
pub const CONTROLLER: &str = "NVMe";

/// Where VBoxManage is: `VBOXMANAGE`, the default install, or the PATH.
pub fn program() -> PathBuf {
    if let Some(path) = std::env::var_os("VBOXMANAGE") {
        return PathBuf::from(path);
    }
    let default = PathBuf::from(r"C:\Program Files\Oracle\VirtualBox\VBoxManage.exe");
    match default.exists() {
        true => default,
        false => PathBuf::from("VBoxManage"),
    }
}

/// Runs VBoxManage with `args`; its stdout, or why it failed.
pub fn run(args: &[&str]) -> Result<String, String> {
    let output = Command::new(program())
        .args(args)
        .output()
        .map_err(|e| format!("cannot run VBoxManage (is VirtualBox installed?): {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(format!(
        "VBoxManage {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// Runs VBoxManage and prints the command first.
pub fn step(args: &[&str]) -> Result<String, String> {
    println!("xtask: VBoxManage {}", args.join(" "));
    run(args)
}

/// VirtualBox's version, if VBoxManage runs.
pub fn version() -> Option<String> {
    run(&["--version"]).ok().map(|v| v.trim().to_string())
}

/// `showvminfo --machinereadable` as keys and unquoted values, or None when the
/// VM is not registered.
pub fn vm_info() -> Option<HashMap<String, String>> {
    let text = run(&["showvminfo", vm(), "--machinereadable"]).ok()?;
    Some(parse_info(&text))
}

pub fn parse_info(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            let unquote = |s: &str| s.trim().trim_matches('"').to_string();
            (unquote(key), unquote(value))
        })
        .collect()
}

/// One registered hard disk, from `list hdds`.
#[derive(Debug, PartialEq, Eq)]
pub struct Hdd {
    pub uuid: String,
    pub parent: String,
    pub location: String,
}

pub fn parse_hdds(text: &str) -> Vec<Hdd> {
    let mut hdds = Vec::new();
    let mut fields: HashMap<&str, &str> = HashMap::new();
    let mut flush = |fields: &mut HashMap<&str, &str>| {
        if let (Some(uuid), Some(parent), Some(location)) = (
            fields.get("UUID"),
            fields.get("Parent UUID"),
            fields.get("Location"),
        ) {
            hdds.push(Hdd {
                uuid: uuid.to_string(),
                parent: parent.to_string(),
                location: location.to_string(),
            });
        }
        fields.clear();
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        // Each disk's block starts with its UUID.
        if key == "UUID" {
            flush(&mut fields);
        }
        fields.insert(key.trim(), value.trim());
    }
    flush(&mut fields);
    hdds
}

/// Unregisters the disk at `location` and the differencing disks below it, which
/// an immutable disk gets one of per boot. Those children are deleted, but only
/// when they lie in `owned` (the VM's folder); the disk itself is only
/// unregistered, never deleted, since its extent may be the model file.
pub fn forget(location: &Path, owned: &Path) -> Result<(), String> {
    forget_below(location, owned, false)
}

/// Unregisters only the differencing disks below the disk at `location`, as
/// [`forget`] does, keeping the disk itself registered: each boot of an immutable
/// disk leaves one behind.
pub fn forget_children(location: &Path, owned: &Path) -> Result<(), String> {
    forget_below(location, owned, true)
}

fn forget_below(location: &Path, owned: &Path, keep_base: bool) -> Result<(), String> {
    let hdds = parse_hdds(&run(&["list", "hdds"])?);
    let Some(base) = hdds.iter().find(|hdd| Path::new(&hdd.location) == location) else {
        return Ok(());
    };
    let mut doomed = vec![base.uuid.clone()];
    let mut i = 0;
    while i < doomed.len() {
        let parent = doomed[i].clone();
        doomed.extend(
            hdds.iter()
                .filter(|hdd| hdd.parent == parent)
                .map(|hdd| hdd.uuid.clone()),
        );
        i += 1;
    }
    // Children first: a disk with children cannot be closed.
    for uuid in doomed.iter().skip(1).rev() {
        step(&["closemedium", "disk", uuid])?;
        let Some(hdd) = hdds.iter().find(|hdd| &hdd.uuid == uuid) else {
            continue;
        };
        // closemedium --delete leaves an immutable disk's children in place.
        let child = Path::new(&hdd.location);
        if child.starts_with(owned) && child.exists() {
            std::fs::remove_file(child)
                .map_err(|e| format!("cannot delete {}: {e}", child.display()))?;
        }
    }
    if keep_base {
        return Ok(());
    }
    step(&["closemedium", "disk", &base.uuid]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdd_lists_parse() {
        let text = "UUID:           1b46\r\nParent UUID:    base\r\nState:          inaccessible\r\nLocation:       D:\\kai-vbox\\kai-model.vmdk\r\n\r\nUUID:           5763\r\nParent UUID:    1b46\r\nLocation:       D:\\kai-vbox\\Kernel AI\\Snapshots\\{5763}.vmdk\r\n";
        let hdds = parse_hdds(text);
        assert_eq!(hdds.len(), 2);
        assert_eq!(hdds[0].uuid, "1b46");
        assert_eq!(hdds[0].parent, "base");
        assert_eq!(hdds[0].location, r"D:\kai-vbox\kai-model.vmdk");
        assert_eq!(hdds[1].parent, "1b46");
    }

    #[test]
    fn machinereadable_info_parses() {
        let info = parse_info(
            "name=\"Kernel AI\"\nVMState=\"poweroff\"\nstoragecontrollername0=\"NVMe\"\n\"NVMe-0-0\"=\"D:\\kai-vbox\\kai-os.vdi\"\n",
        );
        assert_eq!(info["name"], "Kernel AI");
        assert_eq!(info["VMState"], "poweroff");
        assert_eq!(info["storagecontrollername0"], "NVMe");
        assert_eq!(info["NVMe-0-0"], r"D:\kai-vbox\kai-os.vdi");
    }
}
