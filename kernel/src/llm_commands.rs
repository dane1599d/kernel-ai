//! Terminal commands for the local model: `:setup`, the host commands that set one
//! up, and `:model`, the machine check, the model found and one timed test prompt.
//! The kernel reads only the CPU and its own memory; the model's name, licence and
//! speed come from the llm service, which parses the file in ring 3.

use crate::{llm, log, memory, model};
use llmproto::text::printable;

pub const HELP: &[(&str, &str)] = &[
    (":model", "the machine, the model and a timed test prompt"),
    (":setup", "how to set up a local AI model on the host"),
];

/// What `:setup` prints: the host commands, in order.
const SETUP: &[&str] = &[
    "To set up the local AI model, run these on the host PC, in the Kernel AI folder:",
    "  1. cargo xtask setup",
    "     checks the PC, downloads Qwen3-0.6B (639 MB) and checks its SHA-256; the file goes",
    "     to D:\\kai-models on Windows, ~/kai-models elsewhere, or KAI_MODELS if it is set",
    "  2. cargo xtask run --local qwen3-0.6b",
    "     boots Kernel AI with that model attached; then type: ask <question>",
    "The model runs on this computer's CPU, inside Kernel AI; questions go nowhere else.",
    "Answers take seconds with KVM or WHPX, and minutes when QEMU emulates the CPU.",
];
const LINE_CHARS: usize = 160;

/// Runs `command` if it is one of these; false otherwise.
pub fn run(command: &str, rest: &str) -> bool {
    match (command, rest) {
        ("setup", "") => SETUP.iter().for_each(|line| log!("{line}")),
        ("model", "") => model_command(),
        ("setup" | "model", _) => log!("usage: :{command}"),
        _ => return false,
    }
    true
}

fn model_command() {
    machine();
    let Some(held) = model::get() else {
        match model::rejected() {
            Some(why) => log!("model: the kai-model disk was attached but not kept: {why}"),
            None => {
                log!("model: no model disk is attached");
                SETUP.iter().for_each(|line| log!("{line}"));
            }
        }
        return;
    };
    log!("model disk: {} bytes held in memory", held.len());
    log!("(loading the model's details and timing a test prompt...)");
    match llm::model_report() {
        Ok(report) => {
            for line in report.lines() {
                log!("{}", printable(line, LINE_CHARS, 1));
            }
        }
        Err(error) => log!("model: {error}"),
    }
}

/// The machine check: the hypervisor, the vector state ring 3 gets, and memory.
fn machine() {
    let cpu = cpufeat::Features::detect();
    log!("machine: {}", crate::fpu::hypervisor(&cpu));
    let state = crate::fpu::state_name();
    let tier = if cpu.avx2_fma() {
        "the engine's avx2+fma tier"
    } else {
        "only the engine's baseline tier (no AVX2 and FMA)"
    };
    log!(
        "vector state: {state} on in ring 3 (XCR0 {:#x}), so {tier} can run",
        crate::fpu::xcr0()
    );
    let (free, total) = memory::with(|m| (m.free_frames(), m.total_frames()));
    log!(
        "memory: {} of {} MiB free",
        free * 4 / 1024,
        total * 4 / 1024
    );
}
