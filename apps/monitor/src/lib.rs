//! The System monitor's model (P4 task 9): what the machine is doing, as the
//! read-only OS rows tell it (`processes`, `memory`, `cpus`, `uptime_ms` and
//! the model's state), sorted and selected the way the window or the agent says.
//! It reads and never changes anything itself: stopping a process is the OS's
//! `process_stop`, which asks, queued as an effect like any other row.
//!
//! The agent's rows are `monitor_sort` and `monitor_select` ([`Monitor::act_sort`]
//! and [`Monitor::act_select`]). They move the view and say what they found, so
//! "sort by memory" can name the top process from what the app already holds.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod fake;
pub mod rows;
pub mod view;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;
use rows::{Call, Memory, Model, Proc};

/// The longest message the status line shows, in characters.
pub const MESSAGE_CHARS: usize = 120;
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

enum Asked {
    Processes,
    Memory,
    Cpus,
    Uptime,
    Model,
    Stop(String),
}

pub struct Monitor {
    cpus: Option<u32>,
    uptime_ms: Option<u64>,
    memory: Option<Memory>,
    model: Option<Model>,
    /// In the order [`Monitor::sort`] says.
    procs: Vec<Proc>,
    sort: Sort,
    /// Whether the order is reversed from the sort's natural one.
    flipped: bool,
    selected: Option<u32>,
    status: Status,
    pending: Vec<(u32, Asked)>,
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

impl Monitor {
    pub fn new() -> Self {
        Self {
            cpus: None,
            uptime_ms: None,
            memory: None,
            model: None,
            procs: Vec::new(),
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

    pub fn model(&self) -> Option<&Model> {
        self.model.as_ref()
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

    /// Whether answers are still to come.
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

    fn ask(&mut self, os: &mut dyn Os, call: &Call, what: Asked) {
        let ticket = os.request(call);
        if self.pending.len() == MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push((ticket, what));
    }

    /// Asks the OS for everything the window shows. Five effects, and answers
    /// still to come for an earlier refresh are forgotten.
    pub fn refresh(&mut self, os: &mut dyn Os) {
        self.pending.clear();
        self.status = Status::Ready;
        self.ask(os, &rows::processes(), Asked::Processes);
        self.ask(os, &rows::memory(), Asked::Memory);
        self.ask(os, &rows::cpus(), Asked::Cpus);
        self.ask(os, &rows::uptime(), Asked::Uptime);
        self.ask(os, &rows::model_state(), Asked::Model);
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
        let call = p.exact.then(|| rows::stop(&p.name)).flatten();
        match call {
            Some(call) => self.ask(os, &call, Asked::Stop(p.name)),
            None => self.fail("That name cannot be given to the stop row."),
        }
    }

    /// The result of the effect with `ticket`; whether the window changed.
    pub fn done(&mut self, os: &mut dyn Os, ticket: u32, outcome: Result<String, String>) -> bool {
        let Some(at) = self.pending.iter().position(|(t, _)| *t == ticket) else {
            return false;
        };
        let (_, what) = self.pending.remove(at);
        match (what, outcome) {
            (Asked::Stop(name), Ok(_)) => {
                self.refresh(os);
                self.status = Status::Done(format!("Stopped {}.", tame(&name, 40)));
            }
            (Asked::Stop(_), Err(why)) => self.fail(&why),
            (Asked::Processes, Ok(text)) => match rows::parse_processes(&text) {
                Ok(procs) => {
                    self.procs = procs;
                    self.resort();
                    // A process that is gone is no longer selected.
                    if self.selected().is_none() {
                        self.selected = None;
                    }
                }
                Err(why) => self.fail(&format!("processes: {why}")),
            },
            (Asked::Memory, Ok(text)) => match rows::parse_memory(&text) {
                Ok(memory) => self.memory = Some(memory),
                Err(why) => self.fail(&format!("memory: {why}")),
            },
            (Asked::Cpus, Ok(text)) => match rows::parse_cpus(&text) {
                Ok(cpus) => self.cpus = Some(cpus),
                Err(why) => self.fail(&format!("cpus: {why}")),
            },
            (Asked::Uptime, Ok(text)) => match rows::parse_uptime(&text) {
                Ok(ms) => self.uptime_ms = Some(ms),
                Err(why) => self.fail(&format!("uptime: {why}")),
            },
            // The model row may not exist yet, or the model may be off: the
            // line then says it does not know, and that is not a failure.
            (Asked::Model, Ok(text)) => self.model = rows::parse_model(&text).ok(),
            (Asked::Model, Err(_)) => self.model = None,
            (_, Err(why)) => self.fail(&why),
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
