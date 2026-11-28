//! The self-test dispatch: which check a `test=` boot runs, once the kernel is
//! up and user space has started. Moved whole out of `boot_continue` in
//! `main.rs`, so the next cases' lines do not push that file past 800.

use super::*;

/// Runs the check `test` names; returns for the boots that go on to
/// `boot-ok` (`SelfTest::None`, `SelfTest::Boot`).
pub(crate) fn run(test: SelfTest) {
    match test {
        SelfTest::PageFault => {
            faults::trigger_page_fault();
            fail("page fault did not trap");
        }
        SelfTest::DoubleFault => faults::trigger_double_fault(),
        SelfTest::WriteProtect => {
            let result = memory::selftest::trigger_write_protect();
            fail(result.err().unwrap_or("write-protect test returned"));
        }
        SelfTest::StackOverflow => {
            let result = memory::selftest::trigger_stack_overflow();
            fail(result.err().unwrap_or("stack-overflow test returned"));
        }
        SelfTest::Timer => run_case(timer::sixty_second_check),
        SelfTest::Keyboard => run_case(keyboard::typing_check),
        SelfTest::Pointer => run_case(pointer::self_test),
        SelfTest::Display => run_case(display_tests::check),
        SelfTest::Usb => run_case(usb::self_test),
        SelfTest::Sched => run_case(sched_tests::latency_check),
        SelfTest::Watchdog => run_case(sched_tests::watchdog_check),
        SelfTest::Smp => run_case(smp_tests::all_cpus_check),
        SelfTest::Tlb => run_case(smp_tests::tlb_check),
        SelfTest::Storm(seconds) => run_case(|| smp_tests::storm_check(seconds)),
        SelfTest::Net => run_case(llm::self_test),
        SelfTest::Terminal => run_case(terminal::self_test),
        SelfTest::Shell => run_case(shell_tests::check),
        SelfTest::CPrompt => run_case(cc_tests::check),
        SelfTest::CBuild => run_case(cc_build_tests::check),
        SelfTest::AppBuild { deny } => run_case(|| app_build_tests::check(deny)),
        SelfTest::Scripts => run_case(script_tests::check),
        SelfTest::Act(which) => run_case(|| act_tests::check(which)),
        SelfTest::Live => run_case(live_tests::check),
        SelfTest::Edit => run_case(edit_tests::check),
        SelfTest::Manifest => run_case(manifest_tests::check),
        SelfTest::ManifestGood => run_case(manifest_tests::check_good),
        SelfTest::Blk => run_case(|| blk_tests::check(cmdline())),
        SelfTest::BlkWrite => run_case(|| blk_tests::check_write(cmdline())),
        SelfTest::BlkFlush => run_case(|| blkflush_tests::check(cmdline())),
        SelfTest::Part => run_case(part_tests::check),
        SelfTest::ModelStore { bad } => run_case(|| model_tests::store(cmdline(), bad)),
        SelfTest::Files { .. } => run_case(|| files_tests::check(cmdline())),
        SelfTest::AppSaved { reread } => run_case(|| saved_tests::check(reread)),
        SelfTest::FilesRestart => run_case(files_tests::restart),
        SelfTest::FilesDeadClient => run_case(files_tests::dead_client),
        SelfTest::FilesActions => run_case(files_tests::actions),
        SelfTest::FilesChurn { boot, last } => run_case(|| files_tests::churn(boot, last)),
        SelfTest::PkgStore(step) => run_case(|| pkg_tests::check(step)),
        SelfTest::Model => run_case(|| model_tests::check(cmdline())),
        SelfTest::LocalAsk => run_case(|| local_tests::check(cmdline())),
        SelfTest::LocalScript => run_case(|| local_tests::check_script(cmdline())),
        SelfTest::Latency => run_case(latency_tests::check),
        SelfTest::ProcStop => run_case(stop_tests::check),
        SelfTest::ProcUsage => run_case(usage_tests::check),
        SelfTest::Shm => run_case(shm_tests::check),
        SelfTest::Mem => run_case(mem_tests::check),
        SelfTest::Profile => run_case(profile_tests::check),
        SelfTest::ManyProcesses => run_case(many_tests::check),
        SelfTest::DisplayClaim => run_case(claim_tests::check),
        SelfTest::DeskWindows => run_case(desk_tests::check),
        SelfTest::DeskRestart => run_case(desk_tests::restart),
        SelfTest::DeskAsk => run_case(ask_tests::check),
        SelfTest::AppSuspend => run_case(suspend_tests::check),
        SelfTest::EventWake => run_case(wake_tests::check),
        SelfTest::AppOnDemand => run_case(demand_tests::check),
        SelfTest::DeskBoot => run_case(desk_tests::boot_check),
        SelfTest::DeskPanel => run_case(desk_tests::panel_check),
        SelfTest::DeskWheel => run_case(desk_tests::wheel_check),
        SelfTest::DeskPanelShell => run_case(desk_tests::panel_shell_check),
        SelfTest::DeskCardWake => run_case(card_wake_tests::check),
        SelfTest::DeskCatalog => run_case(catalog_tests::check),
        SelfTest::UiWindow => run_case(ui_tests::check),
        SelfTest::DeskWheelApp => run_case(desk_wheel_app_tests::check),
        SelfTest::UiWindowShow => run_case(show_tests::check),
        SelfTest::AppEditor => run_case(app_editor_tests::check),
        SelfTest::AppEditorFiles => run_case(app_editor_files_tests::check),
        SelfTest::AppEditorBuild => run_case(app_editor_build_tests::check),
        SelfTest::UiMonitor => run_case(app_window_tests::monitor_check),
        SelfTest::UiFiles => run_case(app_window_tests::files_check),
        SelfTest::UiModels => run_case(app_window_tests::models_check),
        SelfTest::UiModelsUse => run_case(models_window_tests::check),
        SelfTest::DeskNap => run_case(nap_tests::check),
        SelfTest::DeskNapBusy => run_case(nap_busy_tests::check),
        SelfTest::KeepRunning { reread } => run_case(|| keep_tests::check(reread)),
        SelfTest::DeskMany => run_case(desk_many_tests::check),
        SelfTest::DeskAppearance => run_case(appearance_tests::check),
        SelfTest::DeskCard => run_case(card_tests::check),
        SelfTest::DeskStanding => run_case(standing_tests::check),
        SelfTest::DeskStandingUndo => run_case(standing_tests::undo_check),
        SelfTest::DeskStandingRefused => run_case(standing_tests::refused_check),
        SelfTest::DeskSends => run_case(desk_sends_tests::check),
        SelfTest::DeskPerf => run_case(desk_perf_tests::check),
        SelfTest::DeskProfile => run_case(desk_profile_tests::check),
        SelfTest::DeskProfileFloor => run_case(profile_floor_tests::check),
        SelfTest::UiKeys { busy } => run_case(|| keys_tests::check(busy)),
        SelfTest::DeskTerm => run_case(term_tests::check),
        SelfTest::AppNotes { reread } => run_case(|| app_notes_tests::check(reread)),
        SelfTest::DeskAgent => run_case(agent_desk_tests::agent_check),
        SelfTest::DeskDefault => run_case(default_tests::check),
        SelfTest::DeskNoMemory => run_case(default_tests::no_memory_check),
        SelfTest::DeskWarming => run_case(default_tests::warming_check),
        SelfTest::AppSettings => run_case(app_settings_tests::check),
        SelfTest::DeskTour => run_case(desk_tour_tests::check),
        SelfTest::FounderWalk => run_case(founder_walk_tests::check),
        SelfTest::PaneStates => run_case(pane_states_tests::check),
        SelfTest::DeskTakeover => run_case(agent_desk_tests::takeover_check),
        SelfTest::DeskReopen => run_case(agent_desk_tests::reopen_check),
        SelfTest::DeskStart => run_case(start_tests::check),
        SelfTest::DeskStartEsc => run_case(start_tests::esc_check),
        SelfTest::Scriptrun => run_case(scriptrun_tests::check),
        SelfTest::Fpu => run_case(fpu_tests::fpu_check),
        SelfTest::Probe => run_case(fpu_tests::probe_check),
        SelfTest::AppBench => run_case(appbench_tests::check),
        SelfTest::TufBench => run_case(tufbench_tests::check),
        SelfTest::Handles => run_case(handle_tests::check),
        SelfTest::IpcRtt => run_case(ipc_rtt_tests::check),
        SelfTest::ShellInterrupt => run_case(interrupt_tests::check),
        SelfTest::AsyncCall => run_case(async_tests::check),
        SelfTest::Threads => run_case(thread_tests::check),
        SelfTest::ThreadsKill => run_case(thread_tests::kill_check),
        SelfTest::ThreadPin => run_case(thread_tests::pin_check),
        SelfTest::RunawayStall => run_case(thread_tests::stall_check),
        SelfTest::Matvec => run_case(compute_tests::matvec_check),
        SelfTest::ComputeKeys => run_case(compute_tests::keys_check),
        SelfTest::Bigmap => run_case(bigmap_tests::check),
        SelfTest::IcountProbe => run_case(|| icount_tests::check(cmdline())),
        SelfTest::Reboot => power::reboot(),
        SelfTest::RebootMemory => run_case(power_tests::check),
        // A fault resets the machine; with `panic=reset` even in test mode.
        SelfTest::PanicReset => {
            if cmdline().split_whitespace().any(|word| word == "what=fail") {
                fail("panic-reset: a failed check, on purpose");
            }
            if cmdline()
                .split_whitespace()
                .any(|word| word == "what=heap-held")
            {
                // A fault inside the allocator: talc's lock stays held on this
                // CPU, so the reset must allocate nothing (bug hunt 4, H4-10).
                x86_64::instructions::interrupts::disable();
                memory::heap::hold_for_test();
                panic!("panic-reset: a panic with the heap locked, on purpose");
            }
            panic!("panic-reset: a panic, on purpose")
        }
        SelfTest::Efi => run_case(efi_tests::check),
        SelfTest::EfiReset => efi::reset(),
        SelfTest::PowerReboot => run_case(efi_tests::power_check),
        SelfTest::PowerOff => run_case(efi_tests::power_off_check),
        SelfTest::Devices => run_case(device_tests::check),
        SelfTest::TuneRevert => run_case(tune_tests::revert_check),
        SelfTest::TuneStall => run_case(tune_stall_tests::check),
        SelfTest::Tune => run_case(tune_tests::shell_check),
        SelfTest::TuneLocal => run_case(|| tune_local_tests::local_check(cmdline())),
        SelfTest::TuneDemo => run_case(tune_local_tests::demo_check),
        SelfTest::TuneAb => run_case(|| tune_ab_tests::check(cmdline())),
        SelfTest::NetSetup => run_case(net_tests::setup_check),
        SelfTest::NetFallback => run_case(net_tests::fallback_check),
        SelfTest::NetStatic => run_case(net_tests::static_check),
        SelfTest::NetDest => run_case(net_tests::dest_check),
        SelfTest::NetStream => run_case(|| net_stream_tests::stream_check(cmdline())),
        SelfTest::NetThroughput => run_case(|| net_throughput_tests::check(cmdline())),
        SelfTest::HttpsMock => run_case(https_tests::check),
        SelfTest::NetDownload { again, .. } => run_case(|| net_download_tests::check(again)),
        SelfTest::FetchService => run_case(https_tests::fetch_check),
        SelfTest::NetGrant => run_case(net_grant_tests::check),
        SelfTest::WaitReads => run_case(wait_reads_tests::check),
        SelfTest::EntropyBoots => run_case(entropy_tests::check),
        SelfTest::Slot => {
            // A slot boot holds `updated`'s record calls until its lever has
            // written: the guard is closed here, before the lever runs.
            if slots::lever_done() {
                fail("slots: a slot boot let updated at the record before its lever ran");
            }
            log!("slots: the record waits for this boot's lever");
            slots::on_boot(true);
            test_exit(ExitCode::Success)
        }
        // The model channel's steps restart into no slot (P4 task 10).
        SelfTest::Update => match model_import_tests::step() {
            Some(step) => run_case(|| model_import_tests::check(step)),
            None => slots::update_case(),
        },
        SelfTest::Probation => slots::probation_case(),
        SelfTest::None => slots::on_boot(false),
        SelfTest::Boot => {}
    }
}
