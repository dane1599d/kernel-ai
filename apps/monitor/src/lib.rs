//! The System monitor's model (P4 task 9): what the machine is doing, from the
//! samples the host hands in ([`Monitor::update`]), sorted and selected the way
//! the window or the agent says. A [`Sample`] is plain numbers (the kernel's
//! `METRICS` and the model's `health`), so there is nothing to parse; this crate
//! turns two samples into each process's CPU share. It changes nothing itself:
//! stopping a process is the OS's `process_stop`, which asks, queued as an effect.
//!
//! The agent's rows are `monitor_sort` and `monitor_select` ([`Monitor::act_sort`]
//! and [`Monitor::act_select`]). They move the view and say what they found, so
//! "sort by memory" can name the top process from what the app already holds.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod calls;
pub mod fake;
pub mod sample;
pub mod view;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use calls::Call;
use core::cmp::Ordering;
use sample::{ModelStatus, PAGE, Sample};

/// The longest message the status line shows, in characters.
pub const MESSAGE_CHARS: usize = 120;
/// Most processes kept from one sample.
pub const MAX_PROCS: usize = 256;
/// Longest process name kept, in characters.
pub const NAME_CHARS: usize = 48;
/// Pages in a MiB.
const PAGES_PER_MIB: u64 = (1 << 20) / PAGE;
/// Most requests waiting for an answer; the oldest is forgotten past this.
const MAX_PENDING: usize = 16;

/// How the app queues an effect (`compsdk::app::request` in the component).
pub trait Os {
    /// Queues `call`; its ticket, which [`Monitor::done`] gets with the result.
    fn request(&mut self, call: &Call) -> u32;
}

/// `text` for one line of the window: control characters become spaces, and it
/// is cut to `max` characters.
pub fn tame(text: &str, max: usize) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    /// The name as the window shows it: no control characters, cut short.
    pub name: String,
    /// Whether `name` is the name the OS gave. `process_stop` takes a name, so
    /// only a process whose name is exact can be stopped from here.
    pub exact: bool,
    /// CPU over the time between the last two samples, in thousandths of one
    /// CPU; unknown until there are two.
    pub cpu_permille: Option<u32>,
    pub mem_kib: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub free_mib: u64,
    pub total_mib: u64,
    pub heap_kib: u64,
}

/// What the table is ordered by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Cpu,
    Memory,
    Name,
    Pid,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Cpu, Sort::Memory, Sort::Name, Sort::Pid];

    /// The word the agent and the buttons use.
    pub fn word(self) -> &'static str {
        match self {
            Sort::Cpu => "cpu",
            Sort::Memory => "memory",
            Sort::Name => "name",
            Sort::Pid => "pid",
        }
    }

    pub fn parse(word: &str) -> Option<Sort> {
        let word = word.trim();
        Self::ALL
            .into_iter()
            .find(|s| s.word().eq_ignore_ascii_case(word))
    }

    /// The order a first press gives: biggest first for the numbers.
    pub fn default_biggest_first(self) -> bool {
        matches!(self, Sort::Cpu | Sort::Memory)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    Done(String),
    Failed(String),
}

/// What the last sample said about each process, for the next one's rates.
struct Baseline {
    uptime_ns: u64,
    procs: Vec<(u32, String, u64)>,
}

pub struct Monitor {
    cpus: Option<u32>,
    uptime_ms: Option<u64>,
    memory: Option<Memory>,
    model: ModelStatus,
    omitted: u32,
    /// In the order [`Monitor::sort`] says.
    procs: Vec<Proc>,
    baseline: Option<Baseline>,
    sort: Sort,
    /// Whether the order is reversed from the sort's natural one.
    flipped: bool,
    selected: Option<u32>,
    status: Status,
    /// Tickets of `process_stop` effects, by the name they were asked for.
    pending: Vec<(u32, String)>,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

/// `a` before `b` for `sort`; an unknown value goes last either way.
fn order(sort: Sort, flipped: bool, a: &Proc, b: &Proc) -> Ordering {
    let biggest_first = sort.default_biggest_first() != flipped;
    let by = |x: Option<u64>, y: Option<u64>| match (x, y) {
        (Some(x), Some(y)) if biggest_first => y.cmp(&x),
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    let key = match sort {
        Sort::Cpu => by(a.cpu_permille.map(u64::from), b.cpu_permille.map(u64::from)),
        Sort::Memory => by(a.mem_kib, b.mem_kib),
        Sort::Pid => by(Some(u64::from(a.pid)), Some(u64::from(b.pid))),
        Sort::Name => {
            let names = (a.name.to_lowercase(), b.name.to_lowercase());
            match flipped {
                true => names.1.cmp(&names.0),
                false => names.0.cmp(&names.1),
            }
        }
    };
    key.then(a.pid.cmp(&b.pid))
}

/// A process's CPU in thousandths of one CPU between two samples `elapsed_ns`
/// apart, at most `cpus` CPUs' worth: `dcpu_ns * 1000 / duptime_ns`. None when
/// its CPU time went backwards (a new process under an old pid).
fn share(before: u64, now: u64, elapsed_ns: u64, cpus: u32) -> Option<u32> {
    let used = now.checked_sub(before)?;
    let permille = u128::from(used) * 1000 / u128::from(elapsed_ns);
    let most = u128::from(cpus.max(1)) * 1000;
    Some(permille.min(most) as u32)
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            cpus: None,
            uptime_ms: None,
            memory: None,
            model: ModelStatus::UNKNOWN,
            omitted: 0,
            procs: Vec::new(),
            baseline: None,
            sort: Sort::Cpu,
            flipped: false,
            selected: None,
            status: Status::Ready,
            pending: Vec::new(),
        }
    }

    pub fn cpus(&self) -> Option<u32> {
        self.cpus
    }

    pub fn uptime_ms(&self) -> Option<u64> {
        self.uptime_ms
    }

    pub fn memory(&self) -> Option<Memory> {
        self.memory
    }

    pub fn model(&self) -> ModelStatus {
        self.model
    }

    /// Processes the kernel's list had no room for.
    pub fn omitted(&self) -> u32 {
        self.omitted
    }

    /// The processes, in the order the table shows.
    pub fn procs(&self) -> &[Proc] {
        &self.procs
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    pub fn flipped(&self) -> bool {
        self.flipped
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Whether a `process_stop` is still to be answered.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The selected process, if it is still in the list.
    pub fn selected(&self) -> Option<&Proc> {
        let pid = self.selected?;
        self.procs.iter().find(|p| p.pid == pid)
    }

    /// The selected process's place in [`Monitor::procs`].
    pub fn selected_index(&self) -> Option<usize> {
        let pid = self.selected?;
        self.procs.iter().position(|p| p.pid == pid)
    }

    fn fail(&mut self, why: &str) {
        self.status = Status::Failed(tame(why, MESSAGE_CHARS));
    }

    /// Takes in a new sample: the numbers it holds, and each process's CPU share
    /// since the last one. A sample no later than the last gives no shares and
    /// keeps the older one as the baseline, so two reads in a moment cannot make
    /// a rate of nothing over nothing.
    pub fn update(&mut self, s: &Sample) {
        self.cpus = Some(s.cpus).filter(|&n| n > 0);
        self.uptime_ms = Some(s.uptime_ns / 1_000_000);
        self.memory = (s.total_pages > 0 && s.free_pages <= s.total_pages).then_some(Memory {
            free_mib: s.free_pages / PAGES_PER_MIB,
            total_mib: s.total_pages / PAGES_PER_MIB,
            heap_kib: s.heap_bytes / 1024,
        });
        self.model = s.model;
        self.omitted = s.omitted;
        let elapsed = self
            .baseline
            .as_ref()
            .and_then(|b| s.uptime_ns.checked_sub(b.uptime_ns))
            .filter(|&dt| dt > 0);
        let before = |p: &sample::ProcSample| {
            let base = self.baseline.as_ref()?;
            let (_, _, cpu_ns) = base
                .procs
                .iter()
                .find(|(pid, name, _)| *pid == p.pid && *name == p.name)?;
            Some(*cpu_ns)
        };
        self.procs = s
            .procs
            .iter()
            .take(MAX_PROCS)
            .map(|p| {
                let shown = tame(&p.name, NAME_CHARS);
                Proc {
                    pid: p.pid,
                    exact: shown == p.name,
                    name: shown,
                    cpu_permille: elapsed
                        .zip(before(p))
                        .and_then(|(dt, was)| share(was, p.cpu_ns, dt, s.cpus)),
                    mem_kib: Some(p.pages.saturating_mul(PAGE / 1024)),
                }
            })
            .collect();
        // Keep the older baseline when no time passed; start a new one when the
        // clock went back (a reboot) or there was none.
        let newer = self
            .baseline
            .as_ref()
            .is_none_or(|b| s.uptime_ns != b.uptime_ns);
        if newer {
            self.baseline = Some(Baseline {
                uptime_ns: s.uptime_ns,
                procs: s
                    .procs
                    .iter()
                    .take(MAX_PROCS)
                    .map(|p| (p.pid, p.name.clone(), p.cpu_ns))
                    .collect(),
            });
        }
        self.resort();
        // A process that is gone is no longer selected.
        if self.selected().is_none() {
            self.selected = None;
        }
    }

    fn resort(&mut self) {
        let (sort, flipped) = (self.sort, self.flipped);
        self.procs.sort_by(|a, b| order(sort, flipped, a, b));
    }

    /// Orders the table by `sort`, in its natural direction.
    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        self.flipped = false;
        self.resort();
    }

    /// A press on a sort button: a new sort, or the same one reversed.
    pub fn press_sort(&mut self, sort: Sort) {
        match sort == self.sort {
            true => self.flipped = !self.flipped,
            false => {
                self.sort = sort;
                self.flipped = false;
            }
        }
        self.resort();
    }

    /// Selects the row at `index`.
    pub fn select_index(&mut self, index: usize) -> bool {
        match self.procs.get(index) {
            Some(p) => {
                self.selected = Some(p.pid);
                true
            }
            None => false,
        }
    }

    /// `monitor_sort`: orders by `by` (`cpu`, `memory`, `name` or `pid`) and
    /// says what comes first, so the answer names the top process.
    pub fn act_sort(&mut self, by: &str) -> Result<String, String> {
        let sort = Sort::parse(by)
            .ok_or_else(|| format!("Sort by cpu, memory, name or pid, not {}.", tame(by, 20)))?;
        self.set_sort(sort);
        let first = self.procs.first().map(view::describe);
        Ok(match first {
            Some(top) => format!("Sorted by {}. First: {top}.", sort.word()),
            None => format!("Sorted by {}. No processes are listed yet.", sort.word()),
        })
    }

    /// `monitor_select`: selects the first process called `name`, ignoring case.
    pub fn act_select(&mut self, name: &str) -> Result<String, String> {
        let found = self
            .procs
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
            .cloned();
        match found {
            Some(p) => {
                self.selected = Some(p.pid);
                Ok(format!("Selected {}.", view::describe(&p)))
            }
            None => Err(format!("No process is called {}.", tame(name, 40))),
        }
    }

    /// Asks the OS to stop the selected process by name. `process_stop` asks
    /// the user first and cannot be undone, and it stops every process of that
    /// name; this only queues it.
    pub fn stop_selected(&mut self, os: &mut dyn Os) {
        let Some(p) = self.selected().cloned() else {
            return self.fail("Pick a process to stop first.");
        };
        let call = p.exact.then(|| calls::stop(&p.name)).flatten();
        let Some(call) = call else {
            return self.fail("That name cannot be given to the stop row.");
        };
        let ticket = os.request(&call);
        if self.pending.len() == MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push((ticket, p.name));
    }

    /// The result of the effect with `ticket`; whether the window changed.
    pub fn done(&mut self, ticket: u32, outcome: Result<String, String>) -> bool {
        let Some(at) = self.pending.iter().position(|(t, _)| *t == ticket) else {
            return false;
        };
        let (_, name) = self.pending.remove(at);
        match outcome {
            Ok(_) => self.status = Status::Done(format!("Stopped {}.", tame(&name, 40))),
            Err(why) => self.fail(&why),
        }
        true
    }
}

impl core::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Monitor")
            .field("procs", &self.procs.len())
            .field("sort", &self.sort)
            .field("selected", &self.selected.map(|p| p.to_string()))
            .finish()
    }
}
