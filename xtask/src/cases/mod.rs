//! The kernel self-test cases. Each boots the kernel once with a command line; the
//! kernel decides pass or fail and reports it through isa-debug-exit. Each lane
//! (ADR 0000 section 4) keeps its cases in a table of its own, joined in [`LANES`].

mod agent;
mod components;
mod devices;
mod kernel;
mod network;

use crate::buildsvc::BuildService;
use crate::disk::Fixture;
use std::time::Duration;

/// A check on the host after a case passed.
pub type AfterCheck = fn(&BuildService) -> Result<(), String>;
/// Changes to the build service's repository before the boot.
pub type Prepare = fn(&BuildService) -> Result<(), String>;

pub struct TestCase {
    pub name: &'static str,
    pub cmdline: &'static str,
    /// QEMU `sendkey` names, `!` and a monitor command, or `@` and a QMP command,
    /// in stages: stage N is sent when the kernel logs "ready for keys" for the
    /// Nth time.
    pub keys: &'static [&'static [&'static str]],
    /// Texts that must all appear on the serial console for the case to pass.
    pub expect: &'static [&'static str],
    /// Boot with a build service (`build=` and `build_token=`) on a fresh snapshot
    /// of the working tree.
    pub build: bool,
    /// Checks on the host once the case has passed, given its build service.
    pub after: Option<AfterCheck>,
    /// Refs to prepare before the boot manifest is chosen.
    pub prepare: Option<Prepare>,
    /// The case needs at least this long, whatever --timeout says: the edit case
    /// waits on builds whose own limits nest inside 300 s.
    pub min_timeout: Option<Duration>,
    /// A fixture attached as the `kai-model` disk, with its length and hash on the
    /// command line.
    pub disk: Option<Fixture>,
    /// Boot with the mock LLM server on the command line (`llm=`). Without it, the
    /// case also fails if the mock gets any request while it runs.
    pub server: bool,
    /// Guest memory in MiB when the case needs more than its disk does (`bigmap`
    /// needs RAM above 4 GiB); `--mem` still wins if larger.
    pub memory_mib: Option<u64>,
    /// Attach the disk as an NVMe controller (`-device nvme`) instead of
    /// virtio-blk.
    pub nvme: bool,
    /// Boot from the GPT disk image (`image/`) built from the ESP, not from QEMU's
    /// directory ESP.
    pub image: bool,
    /// Point the kernel at the mock DNS server (`dns=`), which answers the
    /// `netcore::fixture` zone.
    pub dns: bool,
    /// Boot with a virtio-gpu (`-device virtio-gpu-pci`) and no VGA.
    pub gpu: bool,
    /// Check the screendump the keys take (`screen.rs`) once the case has passed.
    pub screen: Option<crate::screen::Expect>,
    /// USB devices (`usb-kbd`, `usb-tablet`, `usb-mouse`) on a `qemu-xhci`
    /// controller; none and no controller when empty.
    pub usb: &'static [&'static str],
    /// Put the network card behind a PCIe root port, on bus 1, so the kernel
    /// reaches it only by walking the bridge.
    pub bridge: bool,
    /// The case passes by resetting the machine: under `-no-reboot` QEMU then
    /// exits with status 0 instead of the isa-debug-exit code.
    pub resets: bool,
}

pub(crate) const fn case(name: &'static str, cmdline: &'static str) -> TestCase {
    TestCase {
        name,
        cmdline,
        keys: &[],
        expect: &[],
        build: false,
        after: None,
        prepare: None,
        min_timeout: None,
        disk: None,
        server: true,
        memory_mib: None,
        nvme: false,
        image: false,
        dns: false,
        gpu: false,
        screen: None,
        usb: &[],
        bridge: false,
        resets: false,
    }
}

/// "kai" and Enter.
pub(super) const KAI: &[&str] = &["k", "a", "i", "ret"];

/// Every lane's cases, in lane order: `cargo xtask test` runs them all, and
/// `--shard K/N` deals them out in this order. A lane edits only its own table.
const LANES: &[&[TestCase]] = &[
    kernel::CASES,
    devices::CASES,
    agent::CASES,
    components::CASES,
    network::CASES,
];

/// The cases `cargo xtask test` boots.
pub fn test_cases() -> Vec<&'static TestCase> {
    LANES.iter().flat_map(|lane| lane.iter()).collect()
}

/// The M5 exit test: a 10-minute spawn, exit and sleep storm.
pub const SOAK_CASES: &[TestCase] = &[case("storm-10min", "test=storm-600")];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_case_has_one_name_and_one_lane() {
        let cases = test_cases();
        let names: HashSet<&str> = cases.iter().map(|case| case.name).collect();
        assert_eq!(names.len(), cases.len());
        assert_eq!(cases.len(), 53);
    }
}
