//! What one case's boots need besides QEMU: its power-cut delays, the LLM
//! server words on its command line, and its `differs` line.

use crate::qemu;

/// When the power goes in boot `boot` of a case with cuts: 0.2 to 2 s after its
/// line, seeded by the case and the boot, so a failure repeats.
pub(crate) fn cut_delay(name: &str, boot: u32) -> std::time::Duration {
    let mut x = name
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64 ^ u64::from(boot), |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
        | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    std::time::Duration::from_millis(200 + x % 1800)
}

/// Kernel command-line arguments that point the kernel at an LLM server.
pub(crate) fn llm_cmdline(llm: Option<&str>, model: Option<&str>) -> String {
    let mut args = Vec::new();
    if let Some(llm) = llm {
        args.push(format!("llm={llm}"));
    }
    if let Some(model) = model {
        args.push(format!("model={model}"));
    }
    args.join(" ")
}

/// A case's `differs` line: kept from its first boot, and on the next boot it
/// must not be the same (two boots' ENTROPY samples).
pub(crate) fn differs(run: &qemu::TestRun, sample: &mut Option<String>) -> Result<(), String> {
    let Some((text, slot)) = &run.capture else {
        return Ok(());
    };
    let got = slot
        .lock()
        .map_err(|_| "the capture lock was poisoned")?
        .take();
    let Some(got) = got else {
        return Err(format!("FAIL: no kernel line contained {text:?}"));
    };
    match sample.replace(got.clone()) {
        Some(previous) if previous == got => Err(format!(
            "FAIL: both boots printed {text:?} with the same {got:?}"
        )),
        Some(previous) => {
            println!("xtask: {text:?} differed between the boots: {previous} then {got}");
            Ok(())
        }
        None => Ok(()),
    }
}
