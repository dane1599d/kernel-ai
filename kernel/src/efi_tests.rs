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
