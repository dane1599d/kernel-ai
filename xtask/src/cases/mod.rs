//! The kernel self-test cases. Each boots the kernel once with a command line; the
//! kernel decides pass or fail and reports it through isa-debug-exit. Each lane
//! (ADR 0000 section 4) keeps its cases in a table of its own, joined in [`LANES`].

mod agent;
mod apps;
mod appshow;
mod cc;
mod components;
mod desksends;
mod devices;
pub(crate) mod https;
mod kernel;
mod modeldl;
mod modelsuse;
mod napbusy;
mod netgrant;
mod netrows;
mod network;
mod online;
mod panestates;
mod pkg;
pub mod release;
mod storage;
mod throughput;
mod tour;
mod updates;
mod walk;
mod warming;

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
    /// When not empty, stage N is sent when a console line, a program's
    /// included, contains entry N, in place of "ready for keys": a boot with no
    /// `test=` mode logs no such line (the release boot, `release_boot.rs`).
    pub keys_on: &'static [&'static str],
    /// Texts that must all appear on the serial console for the case to pass.
    pub expect: &'static [&'static str],
    /// Tell the kernel the accelerator, `accel=kvm` or `accel=tcg`, for a
    /// case that judges timing differently under TCG (an HTTPS case is told
    /// always).
    pub accel: bool,
    /// Lines the console must show whole, after the reader takes off a serial
    /// stamp: a line still carrying its stamp never matches.
    pub exact: &'static [&'static str],
    /// Texts that fail the case wherever they appear, a program's lines
    /// included: [`FORBID`] unless the case sets its own list.
    pub forbid: &'static [&'static str],
    /// The case passes once every expected text is on the console, and QEMU is
    /// then stopped: a boot with no `test=` mode never reaches isa-debug-exit.
    pub stops: bool,
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
    /// The case boots under KVM only: a step it times (an `undo` within a
    /// 5 s hold) would flake under TCG's emulation.
    pub kvm_only: bool,
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
    /// Point the kernel at the TCP echo server (`echo=`), for the stream case.
    pub echo: bool,
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
    /// Give the network card no MSI-X vectors (`vectors=0`), so the kernel
    /// keeps it polled.
    pub net_polled: bool,
    /// The case passes by resetting the machine: under `-no-reboot` QEMU then
    /// exits with status 0 instead of the isa-debug-exit code. Status 0 passes
    /// only if QMP's SHUTDOWN said `guest-reset` (a power-off exits 0 too), and
    /// the case also forbids [`RESET_FALLBACKS`].
    pub resets: bool,
    /// The case passes only by the guest turning the machine off: QEMU runs
    /// with `-no-shutdown`, so a shutdown pauses it, and once every expected
    /// line is in the monitor must report `shutdown`. A reset (QEMU exits
    /// under `-no-reboot`) or a hang fails it.
    pub powers_off: bool,
    /// A fresh writable `kai-data` disk (ADR 0004).
    pub data: Option<crate::blklog::Data>,
    /// With `data` on a GPT disk: `kai-data`'s size in MiB, when the case
    /// stores more than the 64 MiB one holds (`net-download`).
    pub data_mib: Option<u64>,
    /// A second boot, once this one has passed, on the same data disk.
    pub then: Option<&'static TestCase>,
    /// With `then`: the rest of the first console line containing this text
    /// must differ between the two boots (ENTROPY's sample).
    pub differs: Option<&'static str>,
    /// Attach a virtio-rng (`-device virtio-rng-pci`).
    pub rng: bool,
    /// Boot from the slot disk (`image/slots.rs`, ADR 0008) with a `kai.slot`
    /// in the bundle; a `then` boot keeps the firmware's variable store, so the
    /// boot lever's record and one-shot carry over.
    pub slots: bool,
    /// An update disk (`image/update_disk.rs`) of the next release, attached
    /// read-only as `kai-update`.
    pub update: Option<UpdateDisk>,
    /// A fixture attached read-only as the import disk, `kai-import` (P4 task
    /// 10): a GGUF file the model manager imports, never the boot model.
    pub import: Option<Fixture>,
    /// Power cuts (P2 task 9, plan section 4.2): boot `n` of `cuts` adds
    /// `boot=n` to the command line, waits for [`cut_line`]`(n)` and kills QEMU
    /// a seeded moment later; boot `cuts + 1` adds `last` and must pass.
    pub cuts: u32,
    /// A model put in `kai-data`'s model store before the boot (`xtask fs mkfs`,
    /// then `import --model`), with its length and hash on the command line;
    /// needs `data`.
    pub store: Option<Fixture>,
    /// After the import, the hint's FNV-1a64 is changed (its CRC kept good), so
    /// the kernel must refuse the model `fs` fills.
    pub store_bad_fnv: bool,
    /// One byte of this package's target in the image's repository flipped, the
    /// bundle's digests recomputed, so only its signed metadata can tell (P4
    /// task 1's tamper case).
    pub tampered: Option<&'static str>,
    /// The package mirror the guest reaches at `pkgrepo=` (P4 task 2), signed by
    /// the image's keys, or a tampered one.
    pub mirror: Option<crate::pkgrepo::mirrors::MirrorKind>,
    /// xtask's HTTPS mock for this boot (P5 task 12's harness, [`Https`]).
    pub https: Option<Https>,
    /// With `https`: the package repository the mock also serves as GitHub does
    /// (P5 task 8), the `kind` mirror signed by the image's keys, so `pkg`
    /// installs from it over TLS.
    pub online: Option<crate::pkgrepo::mirrors::MirrorKind>,
    /// The next release, served over TLS by the HTTPS mock as GitHub serves the
    /// update repository (P5 task 10), with `fault`: this build as a slot image,
    /// its metadata signed by the run's test root, the test CA in the image's
    /// repository folder (`dns` is set with it).
    pub online_release: Option<crate::image::update_online::Fault>,
    /// A second boot of a slot case keeps slot B as the boot before left it, so
    /// a download that was stopped can resume from what it had written (P5
    /// task 10); otherwise every boot makes the slots anew from its bundle.
    pub keeps_slot_b: bool,
    /// The POSTs the HTTPS mock must have taken, each `(path, body)`, in order
    /// and no others: empty for a case that checks none.
    pub posts: &'static [(&'static str, &'static str)],
}

/// What the HTTPS mock serves a case: its hosts are `netcore::fixture`'s
/// `MOCK_*` names (`dns=` is added), all reached on port 443 at
/// `MOCK_ADDRESS` through QEMU's forward, and its test CA is the boot bundle's
/// `netroots/1.der`, which a holder of NET reads with BOOT_READ
/// (`user::tls::roots`). A release bundle has no such file.
#[derive(Clone, Copy)]
pub struct Https {
    /// The files, redirects, release assets, models and repository the mock
    /// serves.
    pub content: fn() -> crate::mock_https::Content,
    /// Faults set before the boot, as `(host, path, fault)`; a certificate
    /// fault applies to the whole host, whatever its path.
    pub faults: &'static [(&'static str, &'static str, crate::mock_https::Fault)],
}

/// What a case's update disk holds.
#[derive(Clone, Copy, Debug)]
pub struct UpdateDisk {
    /// One byte of the image is flipped after it was signed.
    pub tampered: bool,
}

/// Lines no case expects, from the founder's second walkthrough: a package
/// service that failed, a line the panel or an app terminal dropped, an answer
/// lost on its way to an app, and the supervisor dying; and the kernel's warning
/// that the plan's boot services leave under 16 MiB of the admission budget
/// (`supervise::budget`), which would otherwise surface later as a service
/// refused NoMemory. A case that provokes one on purpose sets its own `forbid`.
pub const FORBID: &[&str] = &[
    "pkg: FAIL",
    "cannot ask yet",
    "dropped a result",
    "did not reach the app",
    "supervision: supervisor died",
    ROOM_WARNING,
];

/// The kernel's warning that a plan nearly fills the admission budget.
pub const ROOM_WARNING: &str = "of the admission budget left for the plan";

/// What the kernel says when a way to reset the machine did not reset it and
/// it tries the next (`power::reboot`, `efi::reset_now`). Every `resets` case
/// forbids these besides its own `forbid`: under `-no-reboot` the next way's
/// reset ends QEMU with status 0 as well, so the reset a case names could
/// break and the case still pass on the one behind it (bug hunt 5 H5-4). No
/// case asks for the triple fault (`reboot=fault`).
pub const RESET_FALLBACKS: &[&str] = &[
    "did not reset the machine",
    "ResetSystem returned; resetting another way",
    "rebooting by a triple fault",
];

impl TestCase {
    /// Every text that fails this case wherever it appears: its `forbid`, and
    /// [`RESET_FALLBACKS`] for a `resets` case.
    pub fn forbidden(&self) -> Vec<&'static str> {
        let fallbacks: &[&str] = if self.resets { RESET_FALLBACKS } else { &[] };
        self.forbid.iter().chain(fallbacks).copied().collect()
    }
}

/// The kernel line that arms boot `boot`'s power cut (`files_tests::churn`).
/// The kernel logs it only once filecheck has marked `churning-<boot>`, after
/// committing actions on the disk the cut before left, so the cut lands while
/// filecheck is making actions rather than on an idle disk (bug hunt 5 H5-5).
pub fn cut_line(boot: u32) -> String {
    format!("files: boot {boot}: filecheck is making actions")
}

/// Guest memory every case gets at least, in MiB. The admission budget is half
/// the memory free once the boot modules are loaded, so on the 512 MiB base the
/// whole system plan (about 104 MiB of quotas) had about 1 MiB of room, and a
/// slightly bigger image refused the shell (#877). 1024 MiB leaves it hundreds.
pub const CASE_MEMORY_MIB: u64 = 1024;

/// Guest memory for a case that boots a model needing `model_mib`
/// (`kabi::model::guest_mib`): the model's share on [`CASE_MEMORY_MIB`] rather
/// than on the 512 MiB base. llm's quota is sized for the model and the base
/// is left for the other services, so on 512 MiB a whole plan beside a model
/// had about 1 MiB of room (`ui-models-use`, run 43627865986).
pub const fn with_case_room(model_mib: u64) -> u64 {
    model_mib - crate::disk::BASE_MEMORY_MIB + CASE_MEMORY_MIB
}

/// The line `cargo xtask test` prints for `case` when it is not booted
/// under TCG (`kvm_only`), so a run that leaves a case out says so; `None`
/// for a case that boots under any accelerator.
pub(crate) fn tcg_skip(case: &TestCase) -> Option<String> {
    case.kvm_only
        .then(|| format!("xtask: {} skipped under TCG (kvm_only)", case.name))
}

pub(crate) const fn case(name: &'static str, cmdline: &'static str) -> TestCase {
    TestCase {
        name,
        cmdline,
        forbid: FORBID,
        keys: &[],
        keys_on: &[],
        expect: &[],
        accel: false,
        exact: &[],
        stops: false,
        build: false,
        after: None,
        prepare: None,
        min_timeout: None,
        disk: None,
        server: true,
        memory_mib: None,
        kvm_only: false,
        nvme: false,
        image: false,
        dns: false,
        echo: false,
        gpu: false,
        screen: None,
        usb: &[],
        bridge: false,
        net_polled: false,
        resets: false,
        powers_off: false,
        data: None,
        data_mib: None,
        then: None,
        differs: None,
        rng: false,
        slots: false,
        update: None,
        import: None,
        cuts: 0,
        store: None,
        store_bad_fnv: false,
        tampered: None,
        mirror: None,
        https: None,
        online: None,
        online_release: None,
        keeps_slot_b: false,
        posts: &[],
    }
}

/// Whether the HTTPS mock served exactly `posts`, each `(path, body)`, as its
/// POSTs, in order: a case with no `posts` checks nothing.
pub fn check_posts(
    posts: &[(&str, &str)],
    served: &[crate::mock_https::Served],
) -> Result<(), String> {
    if posts.is_empty() {
        return Ok(());
    }
    let got: Vec<(String, String)> = served
        .iter()
        .filter(|s| s.method == "POST")
        .map(|s| {
            let body = s
                .body
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default();
            (s.path.clone(), body.into_owned())
        })
        .collect();
    let want: Vec<(String, String)> = posts
        .iter()
        .map(|(path, body)| (String::from(*path), String::from(*body)))
        .collect();
    match got == want {
        true => Ok(()),
        false => Err(format!(
            "FAIL: the HTTPS mock took POSTs {got:?}, not {want:?}"
        )),
    }
}

/// "kai" and Enter.
pub(super) const KAI: &[&str] = &["k", "a", "i", "ret"];

/// Every lane's cases, in lane order: `cargo xtask test` runs them all, and
/// `--shard K/N` deals them out in this order. A lane edits only its own table.
const LANES: &[&[TestCase]] = &[
    kernel::basics::CASES,
    kernel::display::CASES,
    kernel::apps::CASES,
    kernel::desk::CASES,
    kernel::memory::CASES,
    devices::CASES,
    agent::CASES,
    apps::CASES,
    cc::CASES,
    components::CASES,
    network::CASES,
    https::CASES,
    storage::CASES,
    updates::CASES,
    pkg::CASES,
    netrows::CASES,
    online::CASES,
    netgrant::CASES,
    appshow::CASES,
    napbusy::CASES,
    modeldl::CASES,
    modelsuse::CASES,
    walk::CASES,
    panestates::CASES,
    warming::CASES,
    desksends::CASES,
];

/// The cases `cargo xtask test` boots.
pub fn test_cases() -> Vec<&'static TestCase> {
    LANES.iter().flat_map(|lane| lane.iter()).collect()
}

/// The M5 exit test: a 10-minute spawn, exit and sleep storm.
pub const SOAK_CASES: &[TestCase] = &[case("storm-10min", "test=storm-600")];

/// Cases booted only when `XTASK_CASE` names one: the screenshot tour, and
/// the throughput workflow's `net-throughput`.
pub fn named_only() -> Vec<&'static TestCase> {
    tour::CASES
        .iter()
        .chain(core::iter::once(&throughput::NET_THROUGHPUT))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_kvm_only_case_says_it_is_skipped_under_tcg() {
        let cases = test_cases();
        let sends = cases.iter().find(|case| case.name == "desk-sends").unwrap();
        assert_eq!(
            tcg_skip(sends).as_deref(),
            Some("xtask: desk-sends skipped under TCG (kvm_only)")
        );
        let any = cases.iter().find(|case| !case.kvm_only).unwrap();
        assert_eq!(tcg_skip(any), None);
    }

    #[test]
    fn every_case_has_one_name_and_one_lane() {
        let cases = test_cases();
        let names: HashSet<&str> = cases.iter().map(|case| case.name).collect();
        assert_eq!(names.len(), cases.len());
        // Every lane's table is in LANES: counted from the tables themselves, so a
        // PR that adds a case need not change a shared number here.
        let tables = [
            kernel::basics::CASES,
            kernel::display::CASES,
            kernel::apps::CASES,
            kernel::desk::CASES,
            kernel::memory::CASES,
            devices::CASES,
            agent::CASES,
            apps::CASES,
            cc::CASES,
            components::CASES,
            network::CASES,
            https::CASES,
            storage::CASES,
            updates::CASES,
            pkg::CASES,
            netrows::CASES,
            online::CASES,
            netgrant::CASES,
            appshow::CASES,
            napbusy::CASES,
            modeldl::CASES,
            modelsuse::CASES,
            walk::CASES,
            panestates::CASES,
            warming::CASES,
            desksends::CASES,
        ];
        assert_eq!(
            cases.len(),
            tables.iter().map(|lane| lane.len()).sum::<usize>()
        );
        assert!(tables.iter().all(|lane| !lane.is_empty()));
    }

    /// Every case the tree boots: the lanes, the tour, the soak and the
    /// release boot, and each second boot their `then` chains hold.
    fn every_case() -> Vec<&'static TestCase> {
        let mut todo: Vec<&'static TestCase> = test_cases();
        todo.extend(named_only());
        todo.extend(SOAK_CASES.iter());
        todo.push(&release::RELEASE_BOOT);
        let mut all = Vec::new();
        while let Some(case) = todo.pop() {
            todo.extend(case.then);
            all.push(case);
        }
        all
    }

    /// A window's "opened at x,y" line names a cascade spot of deskgeom's
    /// layout, so a layout change fails here before it fails a boot.
    #[test]
    fn every_opened_at_line_is_a_deskgeom_cascade_spot() {
        let layout = deskgeom::points(1280, 800).layout;
        let spots: Vec<String> = (0..deskgeom::CASCADE_SPOTS)
            .map(|n| layout.cascade(n))
            .map(|at| format!("opened at {},{} ", at.x, at.y))
            .collect();
        let mut seen = 0;
        for case in every_case() {
            for text in case.expect {
                let Some(at) = text.find("opened at ") else {
                    continue;
                };
                let rest = &text[at..];
                if rest == "opened at " {
                    continue;
                }
                seen += 1;
                assert!(
                    spots.iter().any(|spot| rest.starts_with(spot.as_str())),
                    "{}: {text}",
                    case.name
                );
            }
        }
        assert!(seen > 0);
    }

    /// A case that expects a forbidden text could never pass: it sets its
    /// own `forbid` instead.
    #[test]
    fn no_case_expects_a_text_it_forbids() {
        for case in every_case() {
            let forbidden = case.forbidden();
            for text in case.expect {
                assert!(
                    !forbidden.iter().any(|bad| text.contains(bad)),
                    "{}: expects {text:?}, which it forbids",
                    case.name
                );
            }
        }
    }

    /// Every case forbids the whole base list, the supervisor's death
    /// included: a case with its own `forbid` copies [`FORBID`] and adds to
    /// it. `many-processes` once left the death out, so a supervisor killed
    /// mid-start and restarted still passed it (bug hunt 5 H5-6).
    #[test]
    fn every_case_forbids_the_base_list() {
        for case in every_case() {
            let forbidden = case.forbidden();
            for text in FORBID {
                assert!(
                    forbidden.contains(text),
                    "{}: does not forbid {text:?}",
                    case.name
                );
            }
        }
    }

    /// Each way the kernel says a reset did not take, as it logs it, fails
    /// every `resets` case, and each such case gets QMP for the reason QEMU
    /// stopped: the next way's reset, or a power-off, would otherwise end QEMU
    /// with status 0 and pass it (bug hunt 5 H5-4).
    #[test]
    fn every_reset_case_fails_when_the_reset_it_names_does_not_take() {
        let kernel = [
            include_str!("../../../kernel/src/power.rs"),
            include_str!("../../../kernel/src/efi.rs"),
        ]
        .concat();
        let fell_through = [
            "power: the reset register did not reset the machine",
            "power: the 8042 did not reset the machine",
            "power: rebooting by a triple fault",
            "efi: ResetSystem returned; resetting another way",
        ];
        for line in fell_through {
            assert!(
                kernel.contains(&format!("log!(\"{line}\")")),
                "the kernel no longer logs {line:?}"
            );
        }
        let mut resets = 0;
        for case in every_case().into_iter().filter(|case| case.resets) {
            resets += 1;
            let forbidden = case.forbidden();
            for line in fell_through {
                assert!(
                    forbidden.iter().any(|text| line.contains(text)),
                    "{}: passes on {line:?}",
                    case.name
                );
            }
            let (_, qmp) = crate::qemu::input_args(case).unwrap();
            assert!(qmp.is_some(), "{}: no QMP for its reset verdict", case.name);
        }
        // reboot, reboot-8042, panic-reset, fail-reset, efi-reset,
        // power-reboot, power-fs and the slot boots.
        assert!(resets > 7, "only {resets} reset cases checked");
        // A case that does not reset is not judged on them.
        let plain = case("plain", "test=x");
        assert!(!plain.forbidden().contains(&RESET_FALLBACKS[0]));
    }

    /// `fs-powercut` fails a boot whose churn stopped on an error before the
    /// cut, and its cut is armed only by the line the kernel logs once
    /// filecheck has committed actions after the cut before, so no cut lands
    /// on an idle disk (bug hunt 5 H5-5).
    #[test]
    fn every_powercut_lands_while_filecheck_makes_actions() {
        let cases = test_cases();
        let cut = cases
            .iter()
            .find(|case| case.name == "fs-powercut")
            .unwrap();
        assert!(cut.cuts > 0);
        let forbidden = cut.forbidden();
        let failed = "[filecheck] filecheck: FAIL commit of action 2000005: Error(NoSpace)";
        assert!(forbidden.iter().any(|text| failed.contains(text)));
        // The kernel logs the cut's line only after `churning-<boot>`, which
        // filecheck leaves after its first actions.
        let kernel = include_str!("../../../kernel/src/files_tests.rs");
        let churn = kernel.split("pub fn churn(").nth(1).unwrap();
        let line = cut_line(u32::MAX).replace(&u32::MAX.to_string(), "{boot}");
        let waited = churn.find("wait_for(&format!(\"churning-{boot}\"))");
        let logged = churn.find(&format!("log!(\"{line}\")"));
        assert!(
            matches!((waited, logged), (Some(w), Some(l)) if w < l),
            "the kernel's churn does not log {line:?} after churning-<boot>"
        );
        let filecheck = include_str!("../../../user/src/bin/filecheck/cut.rs");
        assert!(filecheck.contains("mark(home, &format!(\"churning-{boot}\"))"));
        // The last boot, which must pass, shows the line too.
        let last = cut_line(cut.cuts + 1);
        assert!(cut.expect.iter().any(|text| text.contains(last.as_str())));
    }

    /// The kernel drops an `updated.<key>=` word whose key is not in its
    /// `UPDATED_TEST_WORDS`, so a case that passes one the list lacks boots
    /// without it: `update-online-resume`'s `retries=0` was dropped that way and
    /// `updated` tried the cut again.
    #[test]
    fn every_updated_word_a_case_passes_is_one_the_kernel_hands_on() {
        let kernel = include_str!("../../../kernel/src/selftest/updated.rs");
        let list = kernel
            .split("const UPDATED_TEST_WORDS")
            .nth(1)
            .and_then(|rest| rest.split_once("= [")?.1.split_once("];"))
            .map(|(list, _)| list)
            .expect("the kernel lists the updated.<key>= words it hands on");
        let allowed: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
        let mut passed = 0;
        for case in test_cases() {
            let mut link = Some(case);
            while let Some(boot) = link {
                let keys = boot
                    .cmdline
                    .split_whitespace()
                    .filter_map(|word| word.strip_prefix("updated."))
                    .filter_map(|word| word.split_once('=').map(|(key, _)| key));
                for key in keys {
                    passed += 1;
                    assert!(
                        allowed.contains(&key),
                        "{}: updated.{key}= is dropped by the kernel",
                        boot.name
                    );
                }
                link = boot.then;
            }
        }
        // `update-window` passes three and `update-online-resume` one: the walk
        // reaches both, so a rename of the prefix cannot leave it checking nothing.
        assert!(passed >= 4, "only {passed} updated.<key>= words checked");
    }

    /// A model case keeps the model's share and gets the case floor, not the
    /// 512 MiB base, beneath it: 512 MiB more than the model formula.
    #[test]
    fn a_model_case_gets_the_case_floor_under_its_model() {
        let model = crate::disk::memory_mib_for(crate::disk::GGUF_LEN as u64);
        assert_eq!(
            with_case_room(model),
            model + CASE_MEMORY_MIB - crate::disk::BASE_MEMORY_MIB
        );
        assert!(with_case_room(model) >= model + 512);
    }
}
