//! `test=efi-ab`, spike S08-1 (ADR 0008): the A/B/R boot lever on the firmware's
//! real variables. Each boot reads `KaiBootState`, prints it, takes one command
//! from the host on COM1, runs `bootslot::drill` and writes what changed: the
//! record and Limine's `LoaderEntryOneShot`. A panic or a failed check then goes
//! through [`on_fault`] from the fault path. The host (`cargo xtask efi-ab`) runs
//! the same drill on its prediction and checks where every boot lands.

use crate::efi::{self, Guid, KAI, NV_BS_RT};
use crate::{hpet, log, serial};
use bootslot::drill::{self, Command, End};
use bootslot::{Nv, Record, Slot};
use limine::request::ExecutableFileRequest;
use spin::Mutex;

#[used]
#[unsafe(link_section = ".requests")]
static EXECUTABLE: ExecutableFileRequest = ExecutableFileRequest::new();

/// The Boot Loader Interface's vendor GUID, which Limine reads its variables under.
const BLI: Guid = Guid {
    data1: 0x4a67_b082,
    data2: 0x0a4c,
    data3: 0x41cf,
    data4: [0xb6, 0xc7, 0x44, 0x0b, 0x29, 0xbb, 0x8c, 0x4f],
};
const RECORD: &str = "KaiBootState";
const ONE_SHOT: &str = "LoaderEntryOneShot";

/// The state the fault path finishes from, once a boot has passed its checks.
static PENDING: Mutex<Option<(Nv, Slot, u8)>> = Mutex::new(None);

/// What the firmware calls of one boot cost.
#[derive(Default)]
struct Cost {
    sets: u32,
    set_ns: u64,
    max_set_ns: u64,
    get_ns: u64,
}

fn set(cost: &mut Cost, vendor: &Guid, name: &str, data: &[u8]) -> Result<(), efi::Error> {
    let start = hpet::nanos();
    let result = efi::set_variable(vendor, name, NV_BS_RT, data);
    let took = hpet::nanos() - start;
    cost.sets += 1;
    cost.set_ns += took;
    cost.max_set_ns = cost.max_set_ns.max(took);
    match result {
        Err(efi::Error::NotFound) if data.is_empty() => Ok(()),
        other => other,
    }
}

/// The slot Limine loaded this kernel from, as its entry's command line says
/// (`slot=r|a|b`, beside that entry's `boot(N):` path). Limine 12 reports
/// partition 0 for a file loaded through `boot(N):`, so the file's own
/// partition index cannot tell.
fn this_slot() -> Option<Slot> {
    let slot = crate::cmdline()
        .split_whitespace()
        .find_map(|word| word.strip_prefix("slot="));
    match slot {
        Some("r") => Some(Slot::R),
        Some("a") => Some(Slot::A),
        Some("b") => Some(Slot::B),
        other => {
            let partition = EXECUTABLE
                .response()
                .map_or(0, |r| r.executable_file().partition_index);
            log!("efi-ab: no slot on the command line ({other:?}; partition {partition})");
            None
        }
    }
}

/// `name` under `vendor` as ASCII text from UTF-16, or "none".
fn text_variable<'a>(vendor: &Guid, name: &str, out: &'a mut [u8; 64]) -> &'a str {
    let mut raw = [0u8; 128];
    let Ok(len) = efi::get_variable(vendor, name, &mut raw) else {
        return "none";
    };
    let mut n = 0;
    for unit in raw[..len].as_chunks::<2>().0 {
        if unit[0] == 0 || n == out.len() {
            break;
        }
        out[n] = if unit[0].is_ascii_graphic() {
            unit[0]
        } else {
            b'?'
        };
        n += 1;
    }
    core::str::from_utf8(&out[..n]).unwrap_or("?")
}

/// The UTF-16 bytes of a BLI entry id with its terminator.
fn utf16(id: &str, out: &mut [u8; 64]) -> usize {
    let mut n = 0;
    for byte in id.bytes().chain(core::iter::once(0)) {
        out[n] = byte;
        out[n + 1] = 0;
        n += 2;
    }
    n
}

/// Writes what `after` changed from `before`.
fn write(cost: &mut Cost, before: &Nv, after: &Nv) -> Result<(), efi::Error> {
    if !drill::same(&before.record, &after.record) {
        set(cost, &KAI, RECORD, &after.record.encode())?;
    }
    if after.one_shot != before.one_shot {
        match after.one_shot {
            Some(slot) => {
                let mut bytes = [0u8; 64];
                let len = utf16(drill::entry_id(slot), &mut bytes);
                set(cost, &BLI, ONE_SHOT, &bytes[..len])?;
            }
            None => set(cost, &BLI, ONE_SHOT, &[])?,
        }
    }
    Ok(())
}

/// Reads one command line from COM1.
fn command() -> Command {
    loop {
        log!("efi-ab: command?");
        let mut line = [0u8; 32];
        let len = serial::read_line(&mut line);
        let text = core::str::from_utf8(&line[..len]).unwrap_or("");
        match Command::parse(text) {
            Some(command) => return command,
            None => log!("efi-ab: unknown command {text:?}"),
        }
    }
}

/// The record as read: `None` when missing; corrupt records are counted.
fn read_record(cost: &mut Cost) -> (Option<Record>, &'static str) {
    let mut bytes = [0u8; 128];
    let start = hpet::nanos();
    let read = efi::get_variable(&KAI, RECORD, &mut bytes);
    cost.get_ns += hpet::nanos() - start;
    match read {
        Ok(len) => match Record::decode(&bytes[..len]) {
            Some(record) => (Some(record), "ok"),
            None => (None, "corrupt"),
        },
        Err(efi::Error::NotFound) => (None, "none"),
        Err(error) => {
            log!("efi-ab: GetVariable {RECORD}: {error:?}");
            (None, "unreadable")
        }
    }
}

/// One boot of the drill. Never returns: every boot ends in a reset or a hang.
pub fn run() -> ! {
    let Some(slot) = this_slot() else {
        crate::fail("efi-ab: no slot to run in");
    };
    let mut cost = Cost::default();
    let (record, state) = read_record(&mut cost);
    let mut left = [0u8; 64];
    let left = text_variable(&BLI, ONE_SHOT, &mut left);
    let mut selected = [0u8; 64];
    let selected = text_variable(&BLI, "LoaderEntrySelected", &mut selected);
    let mut hex = [0u8; 128];
    if let Some(record) = &record {
        drill::hex(record, &mut hex);
    }
    log!(
        "efi-ab: boot in={} record={state} {} oneshot-left={left} selected={selected}",
        drill::entry_id(slot),
        core::str::from_utf8(if record.is_some() { &hex } else { b"-" }).unwrap_or("?"),
    );
    let mut nv = Nv {
        record: record.unwrap_or_default(),
        one_shot: None,
    };
    let before = nv;
    if record.is_none() {
        drill::install(&mut nv);
        finish(&mut cost, &before, &nv, slot, End::Reset);
    }
    let mut end = drill::run(&mut nv, slot, command());
    while end == End::Stay {
        log!("efi-ab: R stays up");
        end = drill::stay(&mut nv, command());
    }
    finish(&mut cost, &before, &nv, slot, end)
}

fn finish(cost: &mut Cost, before: &Nv, nv: &Nv, slot: Slot, end: End) -> ! {
    if end == End::DieEarly {
        log!("efi-ab: dying before anything is written");
        panic!("efi-ab: panic-early");
    }
    // `boot_seq` counts the record's writes, not boots: bumped only when the
    // record changes anyway, so a normal boot writes the one-shot alone (ADR 0008:
    // one SetVariable per normal boot).
    let mut nv = *nv;
    if !drill::same(&before.record, &nv.record) {
        nv.record.boot_seq = before.record.boot_seq.wrapping_add(1);
    }
    if let Err(error) = write(cost, before, &nv) {
        log!("efi-ab: SetVariable failed: {error:?}");
    }
    if end == End::SetDefault {
        let mut bytes = [0u8; 64];
        let len = utf16("kai-a", &mut bytes);
        let wrote = efi::set_variable(&BLI, "LoaderEntryDefault", NV_BS_RT, &bytes[..len]);
        log!("efi-ab: LoaderEntryDefault=kai-a written: {wrote:?}");
    }
    log!(
        "efi-ab: wrote sets={} set_us={} max_set_us={} get_us={} init_us={} end={end:?}",
        cost.sets,
        cost.set_ns / 1000,
        cost.max_set_ns / 1000,
        cost.get_ns / 1000,
        init_us(),
    );
    match end {
        End::Fault(kind) => {
            *PENDING.lock() = Some((nv, slot, kind));
            if kind == drill::PANIC {
                panic!("efi-ab: panic-late");
            }
            crate::fail("efi-ab: a required check failed, on purpose");
        }
        End::Hang => {
            log!("efi-ab: hanging with interrupts off");
            crate::halt_forever()
        }
        _ => efi::reset(),
    }
}

/// How long `efi::init` took, from its TSC ticks and a 10 ms TSC measurement.
fn init_us() -> u64 {
    // SAFETY: RDTSC has no side effects.
    let tsc = || unsafe { core::arch::x86_64::_rdtsc() };
    let (start_ns, start_tsc) = (hpet::nanos(), tsc());
    while hpet::nanos() - start_ns < 10_000_000 {
        core::hint::spin_loop();
    }
    let hz = u128::from(tsc() - start_tsc) * 1_000_000_000 / u128::from(hpet::nanos() - start_ns);
    (u128::from(efi::init_ticks()) * 1_000_000 / hz.max(1)) as u64
}

/// From the fault path: records the fault, deletes the one-shot and resets
/// through the firmware. Does nothing unless a drill boot is faulting.
pub fn on_fault() {
    let Some(pending) = PENDING.try_lock().and_then(|mut p| p.take()) else {
        return;
    };
    let (before, slot, kind) = pending;
    let mut after = before;
    bootslot::fault(&mut after, slot, kind, 0);
    let mut cost = Cost::default();
    let wrote = write(&mut cost, &before, &after);
    log!(
        "efi-ab: fault written ({wrote:?}, sets={}, set_us={})",
        cost.sets,
        cost.set_ns / 1000
    );
    efi::reset()
}
