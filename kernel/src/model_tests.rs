//! `test=model`: the GGUF file on the `kai-model` disk is held in memory and mapped
//! into the `llm` service.
//!
//! The kernel stands in for the supervisor, so that at the end every process is
//! gone: it starts `llm` with `cap::MODEL`, whose `health` reports the length and
//! FNV-1a hash of what it mapped; kills it and starts it again, and the new process
//! maps the same bytes; then starts it without the capability, and nothing is
//! mapped. With every process gone, `USER_FRAMES` is 0, while the model's runs are
//! still allocated and still hold the file, every byte after it in the last 2 MiB
//! run is zero, and the window's page tables hold exactly what was built (L6).

use crate::memory::user::USER_FRAMES;
use crate::process::{self, SpawnRequest};
use crate::{hpet, llm, log, memory, model, threads};
use alloc::format;
use alloc::string::String;
use core::sync::atomic::Ordering;
use kabi::model::{Attached, Fnv1a};
use kabi::{ExitReason, Limits, cap};
use mm::PAGE_SIZE;

const EXIT_TIMEOUT_MS: u64 = 10_000;
/// The llm plan line's burst limit.
const BURST_MS: u64 = 10_000;

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let attached = Attached::from_cmdline(cmdline);
    let (Some(len), Some(fnv)) = (attached.len, attached.fnv) else {
        return Err("model: the command line has no model_len= and model_fnv=");
    };
    let model = model::get().ok_or("model: the kai-model disk was not loaded")?;
    if model.len() != len || frames_hash(model) != fnv {
        return Err("model: the frames do not hold the file xtask attached");
    }
    let expected = format!("model {len} bytes fnv1a64 {fnv:016x}");
    let first = start_and_ask(cap::SERVE | cap::MODEL)?;
    let shown = first.1.as_deref().unwrap_or("");
    log!("model: llm (pid {}) health: {shown}", first.0);
    if !shown.starts_with(&expected) {
        return Err("model: llm does not report the file's length and hash");
    }
    stop(first.0)?;
    let second = start_and_ask(cap::SERVE | cap::MODEL)?;
    if second
        .1
        .as_deref()
        .is_none_or(|text| !text.starts_with(&expected))
    {
        return Err("model: the restarted llm does not map the same bytes");
    }
    log!(
        "model: restarted llm (pid {}) maps the same bytes",
        second.0
    );
    stop(second.0)?;
    let refused = start_and_ask(cap::SERVE)?;
    if refused.1.is_some() {
        return Err("model: llm without cap::MODEL still reports a model");
    }
    log!(
        "model: llm (pid {}) without cap::MODEL maps nothing",
        refused.0
    );
    stop(refused.0)?;
    let user_frames = USER_FRAMES.load(Ordering::SeqCst);
    let held = memory::with(|m| model.frames().all(|phys| m.frame_in_use(phys)));
    log!(
        "model: every process gone; USER_FRAMES {user_frames}, model frames {}",
        if held { "still allocated" } else { "freed" }
    );
    if user_frames != 0 {
        return Err("model: user frames leaked");
    }
    if !held || frames_hash(model) != fnv {
        return Err("model: a process's teardown touched the model's frames");
    }
    model::check_window()?;
    log!(
        "model: {} runs of 2 MiB, window of {} page tables intact after every teardown",
        model.run_count(),
        kabi::model::table_pages(model.len())
    );
    log!("model: mapped, remapped after a restart, refused without the capability, never freed");
    Ok(())
}

/// The FNV-1a hash of the model's bytes, read from its runs, or 0 when the rest
/// of the last run, which ring 3 can read too, is not all zero.
fn frames_hash(model: &model::Model) -> u64 {
    let hhdm = memory::hhdm();
    let mut hash = Fnv1a::new();
    let mut left = model.len();
    for phys in model.frames() {
        let take = left.min(PAGE_SIZE) as usize;
        // SAFETY: model frames are allocated for good, mapped by the direct map,
        // and never written after the boot copy.
        let page =
            unsafe { core::slice::from_raw_parts((hhdm + phys) as *const u8, PAGE_SIZE as usize) };
        let (bytes, rest) = page.split_at(take);
        if rest.iter().any(|&byte| byte != 0) {
            return 0;
        }
        hash.update(bytes);
        left -= take as u64;
    }
    hash.finish()
}

/// Starts `llm` with `caps` and asks it for `health`: its pid, and the model part
/// of the reply, `None` when it reports no model.
fn start_and_ask(caps: u64) -> Result<(u64, Option<String>), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: model::HOLDER,
        program: model::HOLDER,
        args: b"",
        limits: Limits {
            max_pages: model::HOLDER_PAGES,
            max_burst_ms: BURST_MS,
            caps,
            stack_pages: 0,
        },
        spawner: 0,
        supervisor: false,
    })
    .map_err(|_| "model: cannot start llm")?;
    match llm::health() {
        Ok(text) if text.starts_with("model ") => Ok((pid, Some(text))),
        // With no server configured for it and no model, llm says so.
        Err(text) if text.starts_with("no LLM server configured") => Ok((pid, None)),
        Ok(text) | Err(text) => {
            log!("model: llm (pid {pid}) health: {text}");
            Err("model: llm's health reply is not the expected one")
        }
    }
}

/// Kills `pid`, waits until its slot is free and its frames are back, and checks
/// its teardown left the model's window intact.
fn stop(pid: u64) -> Result<(), &'static str> {
    process::kill(pid, ExitReason::Killed).map_err(|_| "model: cannot kill llm")?;
    let deadline = hpet::nanos() + EXIT_TIMEOUT_MS * 1_000_000;
    while hpet::nanos() < deadline {
        let mut live = false;
        process::services(|other, _| live |= other == pid);
        if !live {
            return model::check_window();
        }
        threads::sleep_ms(10);
    }
    Err("model: llm did not exit")
}
