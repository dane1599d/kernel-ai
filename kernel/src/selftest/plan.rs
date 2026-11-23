//! The service plan each boot starts with.

use super::updated::updated_words;
use super::{ActCase, SelfTest};
use alloc::format;
use alloc::string::String;

/// The service plan for this boot, or None for the boots that run no user space.
/// Not for `test`: its exact frame and heap leak checks must not race processes
/// starting up.
pub fn plan(test: SelfTest) -> Option<String> {
    let wanted = matches!(
        test,
        SelfTest::None
            | SelfTest::Slot
            | SelfTest::Update
            | SelfTest::Probation
            | SelfTest::Net
            | SelfTest::Terminal
            | SelfTest::Shell
            | SelfTest::CPrompt
            | SelfTest::CBuild
            | SelfTest::Scripts
            | SelfTest::Act(_)
            | SelfTest::Live
            | SelfTest::AppSuspend
            | SelfTest::EventWake
            | SelfTest::AppOnDemand
            | SelfTest::Edit
            | SelfTest::Manifest
            | SelfTest::ManifestGood
            | SelfTest::LocalAsk
            | SelfTest::LocalScript
            | SelfTest::Latency
            | SelfTest::ProcStop
            | SelfTest::ProcUsage
            | SelfTest::Part
            | SelfTest::ModelStore { .. }
            | SelfTest::ManyProcesses
            | SelfTest::Files { .. }
            | SelfTest::AppSaved { .. }
            | SelfTest::FilesRestart
            | SelfTest::FilesDeadClient
            | SelfTest::PkgStore(_)
            | SelfTest::NetDownload { .. }
            | SelfTest::DisplayClaim
            | SelfTest::DeskWindows
            | SelfTest::DeskRestart
            | SelfTest::DeskAsk
            | SelfTest::FilesActions
            | SelfTest::FilesChurn { .. }
            | SelfTest::DeskBoot
            | SelfTest::DeskPanel
            | SelfTest::DeskWheel
            | SelfTest::DeskPanelShell
            | SelfTest::DeskCardWake
            | SelfTest::DeskCatalog
            | SelfTest::UiWindow
            | SelfTest::DeskWheelApp
            | SelfTest::UiWindowShow
            | SelfTest::AppEditor
            | SelfTest::AppEditorFiles
            | SelfTest::AppEditorBuild
            | SelfTest::UiMonitor
            | SelfTest::UiFiles
            | SelfTest::UiModels
            | SelfTest::UiModelsUse
            | SelfTest::DeskNap
            | SelfTest::DeskNapBusy
            | SelfTest::KeepRunning { .. }
            | SelfTest::DeskAppearance
            | SelfTest::DeskCard
            | SelfTest::DeskStanding
            | SelfTest::DeskStandingRefused
            | SelfTest::DeskPerf
            | SelfTest::DeskProfile
            | SelfTest::DeskProfileFloor
            | SelfTest::UiKeys { .. }
            | SelfTest::DeskTerm
            | SelfTest::AppNotes { .. }
            | SelfTest::DeskAgent
            | SelfTest::DeskDefault
            | SelfTest::DeskNoMemory
            | SelfTest::DeskWarming
            | SelfTest::AppSettings
            | SelfTest::NetGrant
            | SelfTest::WaitReads
            | SelfTest::DeskTour
            | SelfTest::FounderWalk
            | SelfTest::PaneStates
            | SelfTest::DeskTakeover
            | SelfTest::DeskReopen
            | SelfTest::DeskStart
            | SelfTest::DeskStartEsc
            | SelfTest::TuneRevert
            | SelfTest::TuneStall
            | SelfTest::Tune
            | SelfTest::TuneLocal
            | SelfTest::TuneDemo
            | SelfTest::TuneAb
    );
    if !wanted {
        return None;
    }
    // The v0.1 system (`kabi::plan::system`): what the default boot plans, and
    // what `desk-default` boots unchanged. With a build service (`build=`, the
    // edit loop), the boot manifest's headless `notes`, which `:edit notes` and
    // the editor build, and the edit service follow; without one no user sees
    // `notes`, and it would cost 32 MiB of the admission budget at boot. The
    // screenshot tour boots it too, so its pictures are the real boot's, and
    // the founder's walk, with `desktop` logging its bar (`log_bar=1`).
    if matches!(
        test,
        SelfTest::None
            | SelfTest::DeskDefault
            | SelfTest::DeskNoMemory
            | SelfTest::DeskWarming
            | SelfTest::DeskTour
            | SelfTest::FounderWalk
            | SelfTest::PaneStates
    ) {
        let files = crate::block::has_partition(diskmap::KAI_DATA_TYPE);
        let mut plan = kabi::plan::system::plan(&crate::llm::plan_line(), files);
        if matches!(test, SelfTest::FounderWalk | SelfTest::PaneStates) {
            plan = crate::founder_walk_tests::plan(&plan);
        }
        if let Some(edit) = crate::build_service::plan_line() {
            plan.push('\n');
            plan.push_str(&crate::boot_manifest::plan_line());
            plan.push('\n');
            plan.push_str(&edit);
        }
        return Some(plan);
    }
    // The live, edit and manifest cases need only their services, with probation
    // ended by calls; the live case also needs llm for `:why`, and the others the
    // edit service, which syncs with git.
    let mut plan = match test {
        SelfTest::Live => format!("{}\n{}", kabi::plan::NOTES_TEST, crate::llm::plan_line()),
        SelfTest::Part => String::from(kabi::plan::FS),
        SelfTest::AppSuspend => kabi::plan::notes("2", None),
        // notes is listed before the shell, whose call reaches it through the
        // supervisor, never a route.
        SelfTest::Act(ActCase::AppRoute) => format!(
            "{}
{}",
            crate::llm::plan_line(),
            kabi::plan::notes("2", None)
        ),
        SelfTest::AppSaved { .. } => crate::saved_tests::plan(),
        SelfTest::AppOnDemand => crate::demand_tests::plan(),
        // llm first, so it asks for the model while fs still fills it.
        SelfTest::ModelStore { bad: false } => {
            format!("{}\n{}", crate::llm::plan_line(), kabi::plan::FS)
        }
        SelfTest::ModelStore { bad: true } => String::from(kabi::plan::FS),
        SelfTest::Files { reread } => {
            crate::files_tests::plan(if reread { "read" } else { "write" })
        }
        SelfTest::FilesRestart => crate::files_tests::plan("restart"),
        SelfTest::FilesDeadClient => crate::files_tests::plan("hold"),
        SelfTest::FilesActions => crate::files_tests::plan("actions"),
        SelfTest::FilesChurn { boot, .. } => {
            crate::files_tests::plan(&alloc::format!("churn boot={boot}"))
        }
        SelfTest::PkgStore(step) => crate::pkg_tests::plan(step),
        SelfTest::NetDownload { again, small } => crate::net_download_tests::plan(again, small),
        SelfTest::DeskCatalog => crate::catalog_tests::plan(),
        SelfTest::CBuild => crate::cc_build_tests::plan(),
        SelfTest::AppEditorBuild => crate::app_editor_build_tests::plan(),
        SelfTest::Edit | SelfTest::Manifest | SelfTest::ManifestGood => {
            let mut plan = match test {
                SelfTest::Edit => String::from(kabi::plan::NOTES_EDIT_TEST),
                _ => format!(
                    "{} {}",
                    crate::boot_manifest::plan_line(),
                    kabi::plan::CALLS_ONLY
                ),
            };
            if let Some(edit) = crate::build_service::plan_line() {
                plan.push('\n');
                plan.push_str(&edit);
            }
            plan
        }
        _ => crate::llm::plan_line(),
    };
    // The net case checks only the llm path, so it runs without the shell.
    if !matches!(
        test,
        SelfTest::Net
            | SelfTest::Live
            | SelfTest::AppSuspend
            | SelfTest::AppOnDemand
            | SelfTest::Edit
            | SelfTest::Manifest
            | SelfTest::ManifestGood
            | SelfTest::Part
            | SelfTest::AppSaved { .. }
            | SelfTest::ModelStore { .. }
            | SelfTest::Files { .. }
            | SelfTest::FilesRestart
            | SelfTest::FilesDeadClient
            | SelfTest::FilesActions
            | SelfTest::FilesChurn { .. }
            | SelfTest::PkgStore(_)
            | SelfTest::NetDownload { .. }
            | SelfTest::DeskCatalog
            | SelfTest::CBuild
            | SelfTest::AppEditorBuild
    ) {
        // The file system service starts before the shell, once there is a
        // `kai-data` partition for it (ADR 0004).
        // With it, the shell gets `files`, its `Dir` for `/home/user`.
        let shell = match crate::block::has_partition(diskmap::KAI_DATA_TYPE) {
            true => {
                plan.push('\n');
                plan.push_str(kabi::plan::FS);
                kabi::plan::SHELL_FILES
            }
            false => kabi::plan::SHELL,
        };
        // The shell calls `cc` for the `c>` prompt, so cc comes first.
        plan.push('\n');
        plan.push_str(kabi::plan::CC);
        // On the desktop the shell has a route to `desktop`, for the panel:
        // `desktop` is planned first, as a route's target must be.
        let shell = match test {
            SelfTest::DeskPanelShell | SelfTest::DeskCardWake => {
                plan.push('\n');
                plan.push_str(crate::desk_tests::BOOT_PLAN);
                kabi::plan::SHELL_DESK
            }
            // The app is planned before the shell, whose call reaches it through
            // the supervisor, and after `desktop`, which its window needs.
            SelfTest::Act(ActCase::AppTerminal) => {
                plan.push('\n');
                plan.push_str(crate::app_terminal_tests::PLAN);
                kabi::plan::SHELL_DESK
            }
            // Notes saves its state through `fs`, so the shell has `files` too.
            SelfTest::AppNotes { .. } => {
                plan.push('\n');
                plan.push_str(crate::agent_desk_tests::DESK);
                kabi::plan::SHELL_DESK_FILES
            }
            // The Files app's effects are answered by the shell, so its shell has
            // the file system and the route to `desktop` ([`kabi::plan::SHELL_DESK_FILES`]).
            SelfTest::Act(ActCase::AppFiles) => {
                plan.push('\n');
                plan.push_str(&crate::app_files_tests::plan());
                kabi::plan::SHELL_DESK_FILES
            }
            SelfTest::Act(ActCase::AppMonitor) => {
                plan.push('\n');
                plan.push_str(&crate::app_monitor_tests::plan());
                kabi::plan::SHELL_DESK
            }
            SelfTest::DeskAgent
            | SelfTest::DeskTakeover
            | SelfTest::DeskReopen
            | SelfTest::DeskStart
            | SelfTest::DeskStartEsc
            | SelfTest::DeskStandingRefused => {
                plan.push('\n');
                plan.push_str(crate::agent_desk_tests::DESK);
                kabi::plan::SHELL_DESK
            }
            // The editor saves through the shell's file rows, so its shell has `files`
            // and a route to `desktop`; the editor itself is planned after the shell.
            SelfTest::AppEditorFiles => {
                plan.push('\n');
                plan.push_str(crate::app_editor_files_tests::DESKTOP);
                kabi::plan::SHELL_DESK_FILES
            }
            // Settings reads what the shell saved, so its shell has a state
            // directory as well as the desktop.
            SelfTest::AppSettings => {
                plan.push('\n');
                plan.push_str(crate::agent_desk_tests::DESK);
                crate::app_settings_tests::SHELL
            }
            // The shell keeps the app's grants in `/system/state`, and asks
            // about each host on `desktop`'s card.
            SelfTest::NetGrant => {
                plan.push('\n');
                plan.push_str(crate::agent_desk_tests::DESK);
                crate::net_grant_tests::SHELL
            }
            // The test app's effects reach the shell through `desktop`; the
            // shell answers them from `/home/user`.
            SelfTest::WaitReads => {
                plan.push('\n');
                plan.push_str(crate::agent_desk_tests::DESK);
                crate::wait_reads_tests::SHELL
            }
            SelfTest::KeepRunning { .. } => {
                plan.push('\n');
                plan.push_str(crate::keep_tests::PLAN);
                crate::keep_tests::SHELL
            }
            // The window's switches reach the shell through `desktop`, so the
            // shell has its route to it; the window comes after the shell.
            SelfTest::UiModelsUse => {
                plan.push('\n');
                plan.push_str(crate::models_window_tests::DESKTOP);
                kabi::plan::SHELL_DESK_FILES
            }
            // The desktop with its card, before the shell with `files` and
            // `state` (the appearance is saved in `/system/state`) and its
            // route to `desktop`.
            SelfTest::DeskAppearance => {
                plan.push('\n');
                plan.push_str(crate::desk_tests::BOOT_PLAN);
                crate::keep_tests::SHELL
            }
            _ => shell,
        };
        // `updated`, the shell's model channel, comes just before it.
        plan.push('\n');
        plan.push_str(&kabi::plan::with_updated(shell, &updated_words(test)));
    }
    // `act-rule` stops processes the supervisor does not restart, and one
    // service it does.
    if test == SelfTest::Act(ActCase::Rule) {
        plan.push_str("\nspin-a program=spinner burst_ms=3600000 unsupervised\nspin-b program=spinner burst_ms=3600000 unsupervised\nspin-c program=spinner burst_ms=3600000 unsupervised\nspin-d program=spinner burst_ms=3600000");
    }
    // `profile-switch` sets two apps' classes and quotas.
    if test == SelfTest::Act(ActCase::Profile) {
        plan.push_str("\nspin-a program=spinner burst_ms=3600000 unsupervised\nspin-b program=spinner burst_ms=3600000 unsupervised");
    }
    // `act-convo` asks to stop two processes and declines both.
    if test == SelfTest::Act(ActCase::Convo) {
        plan.push_str(
            "
spin-a program=spinner burst_ms=3600000 unsupervised
spin-b program=spinner burst_ms=3600000 unsupervised",
        );
    }
    let workload = crate::workload::plan_lines();
    if !workload.is_empty() {
        plan.push('\n');
        plan.push_str(&workload);
    }
    if test == SelfTest::ProcStop {
        plan.push('\n');
        plan.push_str(crate::stop_tests::PLAN);
    }
    if test == SelfTest::ProcUsage {
        plan.push('\n');
        plan.push_str(crate::usage_tests::PLAN);
    }
    if test == SelfTest::ManyProcesses {
        plan.push_str(&crate::many_tests::plan());
    }
    if test == SelfTest::DisplayClaim {
        plan.push('\n');
        plan.push_str(crate::claim_tests::PLAN);
    }
    if matches!(
        test,
        SelfTest::DeskWindows | SelfTest::DeskRestart | SelfTest::DeskAsk | SelfTest::DeskCard
    ) {
        plan.push('\n');
        plan.push_str(crate::desk_tests::PLAN);
    }
    if matches!(
        test,
        SelfTest::DeskBoot | SelfTest::DeskPanel | SelfTest::DeskWheel
    ) {
        plan.push('\n');
        plan.push_str(crate::desk_tests::BOOT_PLAN);
    }
    if test == SelfTest::UiWindow {
        plan.push('\n');
        plan.push_str(crate::ui_tests::PLAN);
    }
    if test == SelfTest::DeskWheelApp {
        plan.push('\n');
        plan.push_str(crate::desk_wheel_app_tests::PLAN);
    }
    if test == SelfTest::UiWindowShow {
        plan.push('\n');
        plan.push_str(crate::show_tests::PLAN);
    }
    if test == SelfTest::AppEditorFiles {
        plan.push('\n');
        plan.push_str(kabi::plan::EDITOR);
    }
    if test == SelfTest::AppEditor {
        plan.push('\n');
        plan.push_str(&crate::app_editor_tests::plan());
    }
    if test == SelfTest::UiMonitor {
        plan.push('\n');
        plan.push_str(&crate::app_window_tests::monitor_plan());
    }
    if test == SelfTest::UiFiles {
        plan.push('\n');
        plan.push_str(&crate::app_window_tests::files_plan());
    }
    // After the shell, whose `updated` is the window's second route.
    if test == SelfTest::UiModels {
        plan.push('\n');
        plan.push_str(&crate::app_window_tests::models_plan());
    }
    if test == SelfTest::UiModelsUse {
        plan.push('\n');
        plan.push_str(kabi::plan::MODELS_WINDOW);
    }
    if matches!(test, SelfTest::DeskNap | SelfTest::DeskNapBusy) {
        plan.push('\n');
        plan.push_str(crate::nap_tests::PLAN);
    }
    if test == SelfTest::DeskStanding {
        plan.push('\n');
        plan.push_str(crate::standing_tests::PLAN);
    }
    if test == SelfTest::DeskStandingRefused {
        plan.push('\n');
        plan.push_str(crate::standing_tests::NOTES);
    }
    if test == SelfTest::DeskPerf {
        plan.push('\n');
        plan.push_str(crate::desk_perf_tests::PLAN);
    }
    if test == SelfTest::DeskProfile {
        plan.push('\n');
        plan.push_str(crate::desk_profile_tests::PLAN);
    }
    if test == SelfTest::DeskProfileFloor {
        plan.push('\n');
        plan.push_str(crate::profile_floor_tests::PLAN);
    }
    if matches!(test, SelfTest::UiKeys { .. }) {
        plan.push('\n');
        plan.push_str(crate::keys_tests::PLAN);
    }
    if test == SelfTest::DeskTerm {
        plan.push('\n');
        plan.push_str(crate::term_tests::PLAN);
    }
    if matches!(test, SelfTest::AppNotes { .. }) {
        plan.push('\n');
        plan.push_str(&crate::app_notes_tests::notes());
    }
    if matches!(
        test,
        SelfTest::DeskAgent
            | SelfTest::DeskTakeover
            | SelfTest::DeskReopen
            | SelfTest::DeskStart
            | SelfTest::DeskStartEsc
    ) {
        plan.push('\n');
        plan.push_str(crate::agent_desk_tests::APPS);
    }
    if test == SelfTest::AppSettings {
        plan.push('\n');
        plan.push_str(crate::app_settings_tests::APPS);
    }
    // The app whose routes are `desktop` and `fetch`: `with_updated` planned
    // `fetch` before the shell already.
    if test == SelfTest::NetGrant {
        plan.push('\n');
        plan.push_str(crate::net_grant_tests::APPS);
    }
    // The headless test app, whose route is `desktop`.
    if test == SelfTest::WaitReads {
        plan.push('\n');
        plan.push_str(crate::wait_reads_tests::APPS);
    }
    Some(plan)
}
