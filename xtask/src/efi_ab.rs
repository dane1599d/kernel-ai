//! `cargo xtask efi-ab [--boots N] [--kills N] [--seed N] [--accel kvm|tcg]`:
//! spike S08-1 (ADR 0008). Boots the R/A/B disk (`image/abr.rs`) again and
//! again on one OVMF variable store, with reboots allowed. Every boot the kernel
//! (`test=efi-ab`) prints the record it read and asks for a command; this side
//! runs the same `bootslot::drill` on its prediction, so it knows where the next
//! boot must land. Commands mix normal restarts, commits, updates, rollbacks,
//! recovery, panics before and after the checks, failed checks and hangs; on top,
//! `--kills` hard kills of QEMU at random moments, restarted on the same store.
//!
//! It measures what the ADR's pass line asks: landings against the prediction,
//! loops, corrupt records, SetVariable calls per normal boot, the time our
//! firmware calls add, and the round trip of a failed boot through R.

use crate::esp::{self, Module};
use crate::options::{Accel, Options};
use bootslot::drill::{self, Command, End};
use bootslot::{Nv, Record, Slot};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const CMDLINE: &str = "test=efi-ab panic=reset";
const IMAGE: &str = "abr.img";
/// How long a boot that hangs is left before the VM is killed.
const HANG_GRACE: Duration = Duration::from_secs(1);
/// Resets in a row with no boot line: the image cannot boot, so the run fails
/// rather than loop.
const MAX_RESETS_WITHOUT_BOOT: usize = 5;
/// The ADR's pass line.
const MAX_BOOTS_TO_SAFE: usize = 4;
const MAX_ADDED_MS: u64 = 50;
const MAX_ROUND_TRIP: Duration = Duration::from_secs(15);

pub const DEFAULT_BOOTS: usize = 300;

/// A small xorshift generator: the run is repeatable from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

#[derive(Default)]
struct Stats {
    boots: usize,
    landed_as_predicted: usize,
    wrong_landings: Vec<String>,
    record_mismatches: Vec<String>,
    corrupt: usize,
    missing_after_install: usize,
    no_tries_landings: usize,
    one_shot_left: usize,
    default_checks: usize,
    default_ignored: usize,
    kills: usize,
    kill_landings_ok: usize,
    hangs: usize,
    restarts: usize,
    timeouts: usize,
    commands: std::collections::BTreeMap<&'static str, usize>,
    /// SetVariable calls of normal boots: a committed slot restarted.
    normal_sets: Vec<u32>,
    /// Microseconds our firmware calls took per boot: init, reads and writes.
    added_us: Vec<u64>,
    set_us: Vec<u64>,
    max_boots_to_safe: usize,
    round_trips: Vec<Duration>,
    cycle: Vec<Duration>,
}

/// What the current boot is waiting for.
struct Boot {
    /// Commands still to send, one per prompt.
    queue: Vec<Command>,
    normal: bool,
}

struct Vm {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

fn start(qemu: &Path, args: &[String], target_dir: &Path) -> Result<Vm, String> {
    let mut child = crate::wsl::command(qemu)
        .current_dir(target_dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start QEMU: {e}"))?;
    let stdout = child.stdout.take().ok_or("QEMU stdout was not captured")?;
    let stdin = child.stdin.take().ok_or("QEMU stdin was not captured")?;
    let (tx, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line.trim_end_matches('\r').to_string()).is_err() {
                break;
            }
        }
    });
    Ok(Vm {
        child,
        stdin,
        lines,
    })
}

fn stop(vm: &mut Vm) {
    let _ = vm.child.kill();
    let _ = vm.child.wait();
}

fn slot_named(id: &str) -> Option<Slot> {
    Slot::ALL.into_iter().find(|s| drill::entry_id(*s) == id)
}

/// A `key=value` word of a console line.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
}

fn decode_hex(hex: &str) -> Option<Record> {
    let bytes: Option<Vec<u8>> = (0..hex.len() / 2)
        .map(|n| u8::from_str_radix(hex.get(2 * n..2 * n + 2)?, 16).ok())
        .collect();
    Record::decode(&bytes?)
}

/// The command for a boot in an A/B slot, from the prediction.
fn pick(rng: &mut Rng, nv: &Nv, slot: Slot, default_now: bool, hangs: bool) -> Command {
    if default_now {
        return Command::SetDefault;
    }
    let committed = nv.record.slot(slot).successful;
    let table: [(Command, u64); 9] = [
        (Command::Reboot, 20),
        (
            if committed {
                Command::Reboot
            } else {
                Command::Commit
            },
            20,
        ),
        (
            if committed {
                Command::Stage
            } else {
                Command::Reboot
            },
            10,
        ),
        (Command::Rollback, 4),
        (Command::Recovery, 4),
        (Command::PanicEarly, 10),
        (Command::PanicLate, 10),
        (Command::FailCheck, 10),
        (Command::Hang, if hangs { 4 } else { 0 }),
    ];
    let total: u64 = table.iter().map(|(_, w)| w).sum();
    let mut roll = rng.below(total);
    for (command, weight) in table {
        if roll < weight {
            return command;
        }
        roll -= weight;
    }
    Command::Reboot
}

/// The command for R when it stays up: a bootable slot, or an update.
fn pick_in_r(rng: &mut Rng, nv: &Nv) -> Command {
    let bootable: Vec<Slot> = [Slot::A, Slot::B]
        .into_iter()
        .filter(|s| bootslot::may_boot(&nv.record, *s))
        .collect();
    if bootable.is_empty() || rng.below(10) == 0 {
        return Command::Stage;
    }
    Command::Boot(bootable[rng.below(bootable.len() as u64) as usize])
}

fn is_fault(command: Command) -> bool {
    matches!(
        command,
        Command::PanicEarly | Command::PanicLate | Command::FailCheck | Command::Hang
    )
}

/// The drill's state on this side of the serial line.
struct Drill {
    rng: Rng,
    nv: Nv,
    /// Where the next boot may land: one slot, or two after a hard kill.
    allowed: Vec<Slot>,
    stats: Stats,
    boot: Option<Boot>,
    hangs: bool,
    /// When the last fault was sent, and whether R has run since.
    fault_at: Option<(Instant, bool)>,
    boots_since_fault: Option<usize>,
    expect_default_r: bool,
    last_boot_at: Option<Instant>,
}

impl Drill {
    /// A boot line: check the landing and the record, then plan the commands.
    fn on_boot(&mut self, line: &str) {
        let now = Instant::now();
        if let Some(last) = self.last_boot_at.replace(now) {
            self.stats.cycle.push(now - last);
        }
        self.stats.boots += 1;
        let landed = field(line, "in").and_then(slot_named).unwrap_or(Slot::R);
        if self.allowed.contains(&landed) {
            self.stats.landed_as_predicted += 1;
            if self.allowed.len() > 1 {
                self.stats.kill_landings_ok += 1;
            }
        } else {
            self.stats.wrong_landings.push(format!(
                "boot {}: landed in {landed:?}, predicted {:?}",
                self.stats.boots, self.allowed
            ));
        }
        if self.expect_default_r {
            self.expect_default_r = false;
            self.stats.default_checks += 1;
            if landed == Slot::R {
                self.stats.default_ignored += 1;
            }
        }
        if field(line, "oneshot-left") != Some("none") {
            self.stats.one_shot_left += 1;
        }
        self.track_safety(landed, now);
        self.nv.one_shot = None;
        self.nv.record.boot_seq = self.nv.record.boot_seq.wrapping_add(1);
        let read = line
            .split_whitespace()
            .skip_while(|w| !w.starts_with("record="))
            .nth(1)
            .and_then(decode_hex);
        let state = field(line, "record").unwrap_or("?");
        if state == "corrupt" {
            self.stats.corrupt += 1;
        }
        let Some(record) = read else {
            if self.stats.boots > 1 && state == "none" {
                self.stats.missing_after_install += 1;
            }
            drill::install(&mut self.nv);
            self.boot = Some(Boot {
                queue: Vec::new(),
                normal: false,
            });
            self.allowed = vec![self.nv.one_shot.unwrap_or(Slot::R)];
            return;
        };
        if !drill::same(&record, &self.nv.record) && self.allowed.len() == 1 {
            self.stats.record_mismatches.push(format!(
                "boot {}: read {record:?}, predicted {:?}",
                self.stats.boots, self.nv.record
            ));
        }
        if !bootslot::may_boot(&record, landed) {
            self.stats.no_tries_landings += 1;
        }
        self.nv.record = record;
        self.plan(landed);
    }

    /// Fault round trips and the boots it takes to reach R or a committed slot.
    fn track_safety(&mut self, landed: Slot, now: Instant) {
        if let Some(boots) = self.boots_since_fault.as_mut() {
            *boots += 1;
            let safe = landed == Slot::R || self.nv.record.slot(landed).successful;
            if safe {
                self.stats.max_boots_to_safe = self.stats.max_boots_to_safe.max(*boots);
                self.boots_since_fault = None;
            }
        }
        if let Some((at, through_r)) = self.fault_at {
            match (landed, through_r) {
                (Slot::R, _) => self.fault_at = Some((at, true)),
                (_, true) => {
                    self.stats.round_trips.push(now - at);
                    self.fault_at = None;
                }
                (_, false) => self.fault_at = None,
            }
        }
    }

    fn plan(&mut self, slot: Slot) {
        let first = match slot {
            Slot::R => {
                let mut probe = self.nv;
                match drill::run(&mut probe, Slot::R, Command::None) {
                    End::Stay => pick_in_r(&mut self.rng, &probe),
                    _ => Command::None,
                }
            }
            _ => pick(
                &mut self.rng,
                &self.nv,
                slot,
                self.stats.default_checks == 0 && !self.expect_default_r,
                self.hangs,
            ),
        };
        let normal = first == Command::Reboot && self.nv.record.slot(slot).successful;
        let mut queue = vec![first];
        let mut end = drill::run(&mut self.nv, slot, first);
        while end == End::Stay {
            let next = pick_in_r(&mut self.rng, &self.nv);
            queue.push(next);
            end = drill::stay(&mut self.nv, next);
        }
        if let End::Fault(kind) = end {
            bootslot::fault(&mut self.nv, slot, kind, 0);
        }
        if first == Command::SetDefault {
            self.expect_default_r = true;
        }
        for command in &queue {
            *self.stats.commands.entry(command.name()).or_default() += 1;
        }
        if is_fault(first) {
            self.boots_since_fault = Some(0);
            // A hang's trip includes the kill and a new QEMU, so it is not timed.
            if first != Command::Hang {
                self.fault_at = Some((Instant::now(), false));
            }
        }
        queue.reverse();
        self.allowed = vec![self.nv.one_shot.unwrap_or(Slot::R)];
        self.boot = Some(Boot { queue, normal });
    }

    /// The `wrote` line: SetVariable calls and the time our calls took.
    fn on_wrote(&mut self, line: &str) {
        let number = |key| field(line, key).and_then(|v| v.parse::<u64>().ok());
        let (Some(sets), Some(set_us), Some(get_us), Some(init_us)) = (
            number("sets"),
            number("set_us"),
            number("get_us"),
            number("init_us"),
        ) else {
            return;
        };
        self.stats.added_us.push(set_us + get_us + init_us);
        if let Some(max) = number("max_set_us") {
            self.stats.set_us.push(max);
        }
        if self.boot.as_ref().is_some_and(|b| b.normal) {
            self.stats.normal_sets.push(sets as u32);
        }
    }
}

/// The QEMU arguments for the drill: the R/A/B disk, one variable store, reboots.
fn drill_args(firmware: &crate::Firmware, options: &Options, accel: Accel) -> Vec<String> {
    let mut args = crate::qemu::args(firmware, accel, true, None, options.sizing);
    args.retain(|arg| arg != "-no-reboot");
    for arg in args.iter_mut() {
        if arg.starts_with("format=raw,file=fat:") {
            *arg = format!("format=raw,file={IMAGE}");
        }
    }
    args
}

pub fn run(
    qemu: &Path,
    target_dir: &Path,
    kernel: &Path,
    modules: &[Module],
    efi: &Path,
    options: &Options,
    accel: Accel,
) -> Result<(), String> {
    let esp_dir = target_dir.join("esp");
    esp::make_esp(&esp_dir, kernel, modules, efi, CMDLINE)?;
    let bytes = crate::image::abr::build(&esp_dir, &target_dir.join(IMAGE), CMDLINE)?;
    println!("xtask: efi-ab: R/A/B disk {IMAGE}, {} MiB", bytes >> 20);
    // One fresh variable store for the whole run, kept across every restart.
    let firmware = crate::find_firmware(qemu, target_dir)?;
    let args = drill_args(&firmware, options, accel);
    println!("xtask: efi-ab: {} {}", qemu.display(), args.join(" "));
    let mut drill = Drill {
        rng: Rng(options.seed.max(1)),
        nv: Nv::default(),
        allowed: vec![Slot::R],
        stats: Stats::default(),
        boot: None,
        hangs: true,
        fault_at: None,
        boots_since_fault: None,
        expect_default_r: false,
        last_boot_at: None,
    };
    let kill_at: Vec<usize> = {
        let mut rng = Rng(options.seed.wrapping_mul(31).max(1));
        let mut at: Vec<usize> = (0..options.kills)
            .map(|_| 2 + rng.below(options.boots.saturating_sub(2) as u64) as usize)
            .collect();
        at.sort_unstable();
        at
    };
    let started = Instant::now();
    while drill.stats.boots < options.boots {
        let mut vm = start(qemu, &args, target_dir)?;
        drill.stats.restarts += 1;
        session(&mut vm, &mut drill, &kill_at, options)?;
        stop(&mut vm);
    }
    report(&drill.stats, accel, started.elapsed(), options)
}

/// One QEMU process: boots until it is killed, hangs, exits or the run is done.
fn session(
    vm: &mut Vm,
    drill: &mut Drill,
    kill_at: &[usize],
    options: &Options,
) -> Result<(), String> {
    let mut kill_after: Option<Instant> = None;
    // From QEMU's start, then from each boot line.
    let mut since = Instant::now();
    let mut resets = 0;
    loop {
        if let Some(at) = kill_after
            && Instant::now() >= at
        {
            // A hard kill: the next boot lands where the writes got to.
            drill.stats.kills += 1;
            let after = drill.nv.one_shot.unwrap_or(Slot::R);
            drill.allowed = vec![Slot::R, after];
            drill.boot = None;
            return Ok(());
        }
        let line = match vm.lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => {
                if since.elapsed() > options.timeout {
                    println!(
                        "xtask: efi-ab: no boot for {:?}; restarting QEMU",
                        options.timeout
                    );
                    drill.stats.timeouts += 1;
                    if drill.stats.timeouts > MAX_RESETS_WITHOUT_BOOT {
                        return Err(format!(
                            "efi-ab: {} timeouts; the drill cannot go on",
                            drill.stats.timeouts
                        ));
                    }
                    drill.allowed = vec![Slot::R, drill.nv.one_shot.unwrap_or(Slot::R)];
                    return Ok(());
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                println!("xtask: efi-ab: QEMU exited; restarting it");
                return Ok(());
            }
        };
        if !line.contains("efi-ab:") && !line.starts_with("fault:") {
            continue;
        }
        println!("  | {line}");
        if line.starts_with("fault:") {
            resets += 1;
            if resets > MAX_RESETS_WITHOUT_BOOT {
                return Err(format!(
                    "efi-ab: {resets} resets in a row with no boot line; last: {line}"
                ));
            }
        }
        if line.contains("efi-ab: boot in=") {
            since = Instant::now();
            resets = 0;
            drill.on_boot(&line);
            if drill.stats.boots >= options.boots {
                return Ok(());
            }
            if kill_at.contains(&drill.stats.boots) {
                let window = median(&drill.stats.cycle).unwrap_or(Duration::from_secs(5));
                let delay = window.mul_f64(drill.rng.below(1000) as f64 / 1000.0);
                kill_after = Some(Instant::now() + delay);
            }
        } else if line.contains("efi-ab: command?") {
            let command = drill
                .boot
                .as_mut()
                .and_then(|b| b.queue.pop())
                .unwrap_or(Command::None);
            writeln!(vm.stdin, "{}", command.name())
                .and_then(|()| vm.stdin.flush())
                .map_err(|e| format!("cannot write to QEMU: {e}"))?;
        } else if line.contains("efi-ab: wrote ") {
            drill.on_wrote(&line);
        } else if line.contains("efi-ab: hanging") {
            drill.stats.hangs += 1;
            thread::sleep(HANG_GRACE);
            drill.boot = None;
            return Ok(());
        }
    }
}

fn median(values: &[Duration]) -> Option<Duration> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied()
}

fn percentile(values: &[u64], p: usize) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted
        .get((sorted.len() * p / 100).min(sorted.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0)
}

fn report(stats: &Stats, accel: Accel, took: Duration, options: &Options) -> Result<(), String> {
    let timed = accel == Accel::Kvm;
    let max_normal_sets = stats.normal_sets.iter().copied().max().unwrap_or(0);
    let round_trip_max = stats.round_trips.iter().max().copied().unwrap_or_default();
    let added_p50 = percentile(&stats.added_us, 50);
    let added_max = percentile(&stats.added_us, 100);
    println!(
        "xtask: efi-ab: {} boots in {} s under {accel:?}, seed {}, {} QEMU starts",
        stats.boots,
        took.as_secs(),
        options.seed,
        stats.restarts
    );
    println!("xtask: efi-ab: commands {:?}", stats.commands);
    println!(
        "xtask: efi-ab: landings as predicted {}/{}; after hard kills {}/{} kills; hangs {}; silent-VM restarts {}",
        stats.landed_as_predicted,
        stats.boots,
        stats.kill_landings_ok,
        stats.kills,
        stats.hangs,
        stats.timeouts
    );
    println!(
        "xtask: efi-ab: corrupt records {}; record lost after install {}; landings on a slot with no tries {}; one-shot left after Limine {}; record mismatches {}",
        stats.corrupt,
        stats.missing_after_install,
        stats.no_tries_landings,
        stats.one_shot_left,
        stats.record_mismatches.len()
    );
    println!(
        "xtask: efi-ab: LoaderEntryDefault=kai-a ignored {}/{}",
        stats.default_ignored, stats.default_checks
    );
    println!(
        "xtask: efi-ab: SetVariable calls per normal boot: max {max_normal_sets} over {} normal boots",
        stats.normal_sets.len()
    );
    println!(
        "xtask: efi-ab: max boots from a fault to R or a committed slot: {}",
        stats.max_boots_to_safe
    );
    if timed {
        println!(
            "xtask: efi-ab: our firmware calls per boot (init + reads + writes): p50 {} us, max {} us; slowest SetVariable p50 {} us, max {} us",
            added_p50,
            added_max,
            percentile(&stats.set_us, 50),
            percentile(&stats.set_us, 100)
        );
        let trips: Vec<u64> = stats
            .round_trips
            .iter()
            .map(|d| d.as_millis() as u64)
            .collect();
        println!(
            "xtask: efi-ab: fault round trip through R: {} trips, p50 {} ms, max {} ms; median boot cycle {} ms",
            trips.len(),
            percentile(&trips, 50),
            percentile(&trips, 100),
            median(&stats.cycle).unwrap_or_default().as_millis()
        );
    }
    for problem in stats
        .wrong_landings
        .iter()
        .chain(&stats.record_mismatches)
        .take(20)
    {
        println!("xtask: efi-ab: {problem}");
    }
    let mut failed = Vec::new();
    if !stats.wrong_landings.is_empty() {
        failed.push("landings off the prediction");
    }
    if !stats.record_mismatches.is_empty() {
        failed.push("records off the prediction");
    }
    if stats.corrupt > 0 || stats.missing_after_install > 0 {
        failed.push("corrupt or lost records");
    }
    if stats.no_tries_landings > 0 {
        failed.push("a landing on a slot with no tries");
    }
    if stats.max_boots_to_safe > MAX_BOOTS_TO_SAFE {
        failed.push("a fault took too many boots to reach safety");
    }
    if max_normal_sets > 1 {
        failed.push("more than one SetVariable on a normal boot");
    }
    if stats.default_ignored != stats.default_checks {
        failed.push("LoaderEntryDefault was obeyed");
    }
    if timed && added_max > MAX_ADDED_MS * 1000 {
        failed.push("firmware calls added more than 50 ms to a boot");
    }
    if timed && round_trip_max > MAX_ROUND_TRIP {
        failed.push("a round trip through R took more than 15 s");
    }
    if failed.is_empty() {
        println!("xtask: efi-ab: PASS");
        Ok(())
    } else {
        Err(format!("efi-ab: FAIL: {}", failed.join("; ")))
    }
}
