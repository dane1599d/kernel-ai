//! `test=efi` (ADR 0008, lane U): the runtime services `bootslot` will use. A
//! variable under Kernel AI's GUID is written, read back, read with too small a
//! buffer, deleted and found gone; a bad name is refused before the firmware is
//! called. `test=efi-reset` resets the machine through ResetSystem.

use crate::efi::{self, Error, KAI, NV_BS_RT};
use crate::log;

const NAME: &str = "KaiEfiTest";
const VALUE: &[u8] = b"kernel-ai efi";

pub fn check() -> Result<(), &'static str> {
    let mut buf = [0u8; 64];
    let expect = |got: Result<usize, Error>, want: Result<usize, Error>, what| {
        if got == want {
            return Ok(());
        }
        log!("efi: {what}: got {got:?}, wanted {want:?}");
        Err("efi: a variable call gave the wrong answer")
    };
    // Delete a leftover first: OVMF's variable store is fresh each boot in CI,
    // but not everywhere.
    let _ = efi::set_variable(&KAI, NAME, NV_BS_RT, &[]);
    expect(
        efi::get_variable(&KAI, NAME, &mut buf),
        Err(Error::NotFound),
        "before",
    )?;
    efi::set_variable(&KAI, NAME, NV_BS_RT, VALUE).map_err(|error| {
        log!("efi: SetVariable: {error:?}");
        "efi: SetVariable failed"
    })?;
    expect(
        efi::get_variable(&KAI, NAME, &mut buf),
        Ok(VALUE.len()),
        "read back",
    )?;
    if &buf[..VALUE.len()] != VALUE {
        return Err("efi: the variable read back differs");
    }
    expect(
        efi::get_variable(&KAI, NAME, &mut buf[..4]),
        Err(Error::BufferTooSmall(VALUE.len())),
        "a small buffer",
    )?;
    efi::set_variable(&KAI, NAME, NV_BS_RT, &[]).map_err(|_| "efi: deleting failed")?;
    expect(
        efi::get_variable(&KAI, NAME, &mut buf),
        Err(Error::NotFound),
        "after delete",
    )?;
    expect(
        efi::get_variable(&KAI, &"x".repeat(80), &mut buf),
        Err(Error::BadName),
        "a long name",
    )?;
    log!(
        "efi: SetVariable, GetVariable and a delete round-tripped a variable under Kernel AI's GUID"
    );
    Ok(())
}

/// `test=power-reboot` (backlog item 13): POWER_REBOOT from ring 3. The
/// supervisor gives `cap::POWER` to the shell alone. `handles mode=reboot` with
/// no capability is refused and exits 0; with `cap::POWER` (started by the
/// kernel, past the supervisor's rule) it restarts the machine, so the case
/// passes by the reset.
pub fn power_check() -> Result<(), &'static str> {
    use crate::process::supervision::power_allowed;
    let power = kabi::cap::POWER;
    let refused = [("handles", "handles"), ("shell", "handles"), ("x", "shell")];
    if refused
        .iter()
        .any(|(name, program)| power_allowed(name, program, power))
        || !power_allowed("shell", "shell", power)
        || !power_allowed("handles", "handles", 0)
    {
        return Err("power: the supervisor's POWER rule is not the shell's alone");
    }
    let refused = start_reboot("hnopower", 0)?;
    if crate::fpu_tests::wait_exit(refused)? != (kabi::ExitReason::Exited, 0) {
        return Err("power: POWER_REBOOT without POWER was not refused");
    }
    log!(
        "power: POWER_REBOOT without the power capability was refused; POWER is the shell's alone"
    );
    restart()
}

/// Starts `handles mode=reboot` as `name` with `caps`.
fn start_reboot(name: &'static str, caps: u64) -> Result<u64, &'static str> {
    crate::process::spawn(&crate::process::SpawnRequest {
        name,
        program: "handles",
        args: b"mode=reboot",
        limits: kabi::Limits {
            max_pages: 256,
            max_burst_ms: 10_000,
            caps,
            stack_pages: 0,
        },
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "power: cannot start the handles program")
}

/// POWER_REBOOT from ring 3 (`hpower`, holding `cap::POWER`): returns only if
/// the machine did not restart.
pub fn restart() -> Result<(), &'static str> {
    let held = start_reboot("hpower", kabi::cap::POWER)?;
    let got = crate::fpu_tests::wait_exit(held)?;
    log!(
        "power: hpower ended {} ({}) instead of restarting",
        got.0.name(),
        got.1
    );
    Err("power: POWER_REBOOT with POWER did not restart the machine")
}
