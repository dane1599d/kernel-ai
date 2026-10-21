//! The doctor's VirtualBox line: whether VirtualBox is installed, and whether it
//! can run beside Windows' hypervisor. With the hypervisor on (WSL2, WHPX), VirtualBox
//! 6.1 and later run on Windows Hypervisor Platform instead of their own VT-x
//! driver, which works but is slower; older ones do not start a VM at all.

use crate::wsl::check::{Item, Step, fail, pass, run};

/// The item for VirtualBox `version` (None if VBoxManage does not run), with
/// Windows' hypervisor running or not.
pub fn item(version: Option<&str>, hypervisor: Option<bool>) -> Item {
    let name = "VirtualBox (cargo xtask vbox)";
    let Some(version) = version else {
        return fail(
            name,
            "not installed; optional, for running Kernel AI in VirtualBox",
            install(),
        );
    };
    let major: u32 = version
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    match (major, hypervisor) {
        (0..=6, Some(true)) if !version.starts_with("6.1") => fail(
            name,
            format!("{version} cannot run beside Windows' hypervisor, which is on"),
            install(),
        ),
        (_, Some(true)) => pass(
            name,
            format!(
                "{version}; Windows' hypervisor is on, so VirtualBox runs on Hyper-V's \
                 platform (WHP): it works, a little slower than native VT-x"
            ),
        ),
        _ => pass(name, format!("{version}, on its own VT-x driver")),
    }
}

fn install() -> Vec<Step> {
    vec![run("winget install Oracle.VirtualBox")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtualbox_with_and_without_the_hypervisor() {
        assert!(!item(None, Some(true)).ok);
        let whp = item(Some("7.2.16r174877"), Some(true));
        assert!(whp.ok && whp.found.contains("WHP"));
        let native = item(Some("7.2.16r174877"), Some(false));
        assert!(native.ok && native.found.contains("VT-x"));
        assert!(!item(Some("6.0.24r139119"), Some(true)).ok);
        assert!(item(Some("6.1.50r161033"), Some(true)).ok);
    }
}
