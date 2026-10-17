//! M9b self-test (`test=edit`): the shared scenario (`editcore::scenario`) run by
//! the kernel against the real notes host and edit service, building on the host
//! the case's xtask started. It calls the same edit-mode functions the terminal
//! does; every check reads a reply, the kernel's store, ledger and outbox, the
//! BUILD count, the process table or the IPC layer's counts.

use crate::live_tests::{self, SPOOF_PID};
use crate::process::{self, components};
use crate::{build_service, edit_mode, hpet, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use editcore::scenario::{self, Applied, Driver};
use kabi::{Error, cap};
use livecore::ledger::Ledger;
use livecore::scenario::Driver as LiveDriver;
use livecore::store::Component;
use livecore::uploads::SyncJob;

const NAME: &str = "notes";
const START_TIMEOUT_MS: u64 = 60_000;
/// How long git may take to acknowledge the outbox.
const SYNC_TIMEOUT_MS: u64 = 60_000;

struct Kernel {
    live: live_tests::Kernel,
    /// Requests the driver sent the edit service.
    edits: u64,
}

impl LiveDriver for Kernel {
    fn call(&mut self, request: &str) -> Result<String, String> {
        self.live.call(request)
    }

    fn spoof(&mut self, request: &str) -> Result<String, String> {
        self.live.spoof(request)
    }

    fn ledger(&mut self) -> Ledger {
        self.live.ledger()
    }

    fn fetches(&mut self, label: &str) -> u64 {
        self.live.fetches(label)
    }

    fn host_pid(&mut self) -> u64 {
        self.live.host_pid()
    }

    fn restart(&mut self) -> Result<(), String> {
        self.live.restart()
    }

    fn log(&mut self, text: &str) {
        log!("{}", text.replacen("live: ", "edit: ", 1));
    }
}

fn screen() -> (usize, usize) {
    crate::console::size().unwrap_or((usize::MAX, usize::MAX))
}

impl Driver for Kernel {
    fn edit(&mut self, line: &str) -> Result<String, String> {
        if let Some(rest) = line.strip_prefix(":edit ") {
            let mut words = rest.split(' ');
            let name = words.next().unwrap_or("");
            self.edits += 1;
            return Ok(edit_mode::open(name, words.next()));
        }
        if line == ":quit" {
            return Ok(edit_mode::quit());
        }
        if edit_mode::editing().is_none() {
            return Err(format!("{line:?} typed outside edit mode"));
        }
        if edit_mode::builds(line) {
            log!("edit: (building on the host...)");
        }
        self.edits += 1;
        Ok(edit_mode::editor_line(line))
    }

    fn apply(&mut self, answer: &str) -> Result<Applied, String> {
        let (cols, rows) = screen();
        let proposal = match edit_mode::propose(NAME, cols, rows) {
            Ok(proposal) => proposal,
            Err(why) => return Ok(Applied::Refused(why)),
        };
        for line in &proposal.lines {
            log!("? {line}");
        }
        if matches!(answer, "y" | "yes") {
            self.live.calls += 1;
        }
        let outcome = edit_mode::answer(&proposal, answer);
        Ok(Applied::Asked {
            view: proposal.lines.join("\n"),
            outcome,
        })
    }

    fn revert(&mut self, args: &str) -> Result<String, String> {
        let mut words = args.split(' ');
        let name = words.next().unwrap_or("");
        let n = words
            .next()
            .map_or(Ok(1), str::parse)
            .map_err(|_| "bad n")?;
        self.live.calls += 1;
        Ok(edit_mode::revert(name, n))
    }

    fn component(&mut self) -> Component {
        components::component(NAME).unwrap_or_else(|| Component::new(NAME))
    }

    fn builds(&mut self) -> u64 {
        build_service::exchanges()
    }

    fn settle_sync(&mut self) -> Result<Vec<SyncJob>, String> {
        let deadline = hpet::nanos() + SYNC_TIMEOUT_MS * 1_000_000;
        while components::next_sync().is_some() {
            if hpet::nanos() > deadline {
                return Err(format!(
                    "git did not acknowledge within {SYNC_TIMEOUT_MS} ms"
                ));
            }
            threads::sleep_ms(50);
        }
        Ok(self.component().uploads.outbox.synced().cloned().collect())
    }

    fn holders(&mut self) -> Option<(Vec<String>, Vec<String>)> {
        Some((
            process::holders(cap::PUBLISH),
            process::holders(cap::COMPONENT),
        ))
    }

    fn spoof_edit(&mut self, request: &str) -> Result<String, String> {
        let deadline = hpet::nanos() + edit_mode::EDIT_CALL_MS * 1_000_000;
        ipc::call_as("edit", request.as_bytes(), SPOOF_PID, deadline, || false)
            .map(|reply| String::from_utf8_lossy(&reply).into_owned())
            .map_err(|error| format!("{request:?} got no reply: {error:?}"))
    }

    fn put_boot_artifact(&mut self, label: &str) -> Result<(), Error> {
        let component = components::component(NAME).ok_or(Error::NotFound)?;
        let bytes = component.bytes(label)?.as_slice().to_vec();
        let commit = "0".repeat(kabi::live::COMMIT_LEN);
        components::store_upload(NAME, bytes, label, &commit).map(|_| ())
    }
}

fn wait_for_edit() -> Result<(), String> {
    for _ in 0..START_TIMEOUT_MS / 10 {
        if ipc::server_of("edit").is_some() {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    Err(format!("no edit service within {START_TIMEOUT_MS} ms"))
}

pub fn check() -> Result<(), &'static str> {
    if build_service::target().is_none() {
        return Err("edit: boot with build= and build_token=");
    }
    if let Err(why) = live_tests::wait_for_host(0).and_then(|_| wait_for_edit()) {
        log!("{why}");
        return Err("edit: the notes host or the edit service never came up");
    }
    let (notes_before, edit_before) = (ipc::replies(NAME), ipc::replies("edit"));
    let started = hpet::nanos();
    let mut kernel = Kernel {
        live: live_tests::Kernel { calls: 0 },
        edits: 0,
    };
    if let Err(why) = scenario::run(&mut kernel) {
        log!("{why}");
        return Err("edit: the scenario failed");
    }
    // notes answered only the kernel; edit answered the kernel (the driver and the
    // git syncs) and one process, the spoof.
    let (notes, edit) = (ipc::replies(NAME), ipc::replies("edit"));
    let notes = (notes.0 - notes_before.0, notes.1 - notes_before.1);
    let edit = (edit.0 - edit_before.0, edit.1 - edit_before.1);
    if notes != (kernel.live.calls, 0) || edit.1 != 1 || edit.0 < kernel.edits + 3 {
        log!(
            "edit: notes answered {notes:?} and edit {edit:?} (kernel, process) calls; the kernel made {} and {}",
            kernel.live.calls,
            kernel.edits
        );
        return Err("edit: the reply counts are wrong");
    }
    log!(
        "edit: {} steps passed in {} s: edited, built, applied, rolled back, restarted and reverted ({} builds)",
        scenario::STEPS.len(),
        (hpet::nanos() - started) / 1_000_000_000,
        build_service::exchanges()
    );
    Ok(())
}
