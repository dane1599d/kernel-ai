//! The workload mixes M10 measures, as data shared by the simulator, the host
//! rehearsal and, later, the ring-3 `work` program.
//!
//! Mix A is an editor while a build runs. The build jobs compute for 2-4 ms and
//! sleep 1 ms, so Interactive boosts every one of their wakes and the editor's key
//! presses queue behind them. The indexer wakes as often as a fast typist types
//! and runs bursts as short as the editor's, so a rule that looks only at burst
//! length treats it like the editor. Labelling it background would cost it most
//! of its CPU (short, frequent bursts live on the wake boost), which the judge
//! does not allow; the right pick labels the build jobs by name and leaves the
//! indexer alone, while the burst rule, which cannot read names, demotes the
//! builds too far. Two hogs never sleep. Mix B is the control: the editor and six
//! hogs, where Interactive already does as well as anything can.

use crate::judge::Choice;
use crate::sim::{Behavior, MS, ProcSpec, Span, US};
use crate::spec::{PolicyKind, PolicySpec, Preset};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// CPU a hog may use without blocking: an hour, so a hog that never blocks is not
/// killed as a runaway in the middle of a trial (the default is 2 s).
pub const HOG_BURST_MS: u64 = 3_600_000;
/// The limit for the processes that block: ten times their longest run.
pub const WORK_BURST_MS: u64 = 2_000;
/// Pages for one `work` process: its image, stack and a small heap.
pub const WORK_PAGES: u64 = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mix {
    pub name: &'static str,
    pub procs: &'static [ProcSpec],
    /// The process the founder wants responsive.
    pub target: usize,
    /// The designers' pick: `classes/firm` with these labelled background, or
    /// keep (Interactive) when empty.
    pub oracle: &'static [&'static str],
}

const fn proc(name: &'static str, behavior: Behavior) -> ProcSpec {
    ProcSpec { name, behavior }
}

const EDITOR: Behavior = Behavior::Probe {
    gap: Span::new(20 * MS, 60 * MS),
    burst: Span::new(200 * US, 800 * US),
};
const BUILD: Behavior = Behavior::Wakeful {
    run: Span::new(2 * MS, 4 * MS),
    sleep: Span::new(MS, MS),
};
const INDEXER: Behavior = Behavior::Wakeful {
    run: Span::new(300 * US, 600 * US),
    sleep: Span::new(MS, 2 * MS),
};

pub const MIX_A: Mix = Mix {
    name: "A",
    procs: &[
        proc("editor", EDITOR),
        proc("build-1", BUILD),
        proc("build-2", BUILD),
        proc("build-3", BUILD),
        proc("build-4", BUILD),
        proc("indexer", INDEXER),
        proc("hog-1", Behavior::Hog),
        proc("hog-2", Behavior::Hog),
    ],
    target: 0,
    oracle: &["build-1", "build-2", "build-3", "build-4"],
};

pub const MIX_B: Mix = Mix {
    name: "B",
    procs: &[
        proc("editor", EDITOR),
        proc("hog-1", Behavior::Hog),
        proc("hog-2", Behavior::Hog),
        proc("hog-3", Behavior::Hog),
        proc("hog-4", Behavior::Hog),
        proc("hog-5", Behavior::Hog),
        proc("hog-6", Behavior::Hog),
    ],
    target: 0,
    oracle: &[],
};

pub const MIXES: [Mix; 2] = [MIX_A, MIX_B];

impl Mix {
    pub fn by_name(name: &str) -> Option<Mix> {
        MIXES
            .into_iter()
            .find(|mix| mix.name.eq_ignore_ascii_case(name))
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.procs.iter().position(|proc| proc.name == name)
    }

    /// The labels mask for the named processes; `None` if a name is unknown or
    /// past the mask's 32 processes.
    pub fn mask(&self, names: &[&str]) -> Option<u32> {
        names.iter().try_fold(0u32, |mask, name| {
            let index = self.index(name).filter(|&i| i < 32)?;
            Some(mask | 1 << index)
        })
    }

    /// The designers' pick as a choice.
    pub fn oracle_choice(&self) -> Choice {
        match self
            .mask(self.oracle)
            .expect("a mix's oracle names its own processes")
        {
            0 => Choice::DEFAULT,
            mask => Choice::new(PolicySpec::new(PolicyKind::Classes, Preset::Firm), mask),
        }
    }

    /// Processes whose label can change anything: those that ever wake, apart
    /// from the target. A hog never wakes, so a label on it is a no-op.
    pub fn labelable(&self) -> impl Iterator<Item = usize> + '_ {
        self.procs
            .iter()
            .enumerate()
            .filter(move |&(index, proc)| index != self.target && proc.behavior != Behavior::Hog)
            .map(|(index, _)| index)
    }
}

impl Mix {
    /// Supervisor plan lines that run the mix as ring-3 `work` processes, seeded
    /// with `seed`. On more than one CPU, one more hog per extra CPU keeps the
    /// CPUs contended, as long as the mix stays within `room` processes.
    pub fn plan_lines(&self, seed: u64, cpus: usize, room: usize) -> Vec<String> {
        let mut lines: Vec<String> = self
            .procs
            .iter()
            .enumerate()
            .map(|(index, proc)| self.plan_line(proc.name, index, seed))
            .collect();
        let hogs: Vec<usize> = (0..self.procs.len())
            .filter(|&i| self.procs[i].behavior == Behavior::Hog)
            .collect();
        if let Some(&hog) = hogs.first() {
            for extra in 0..cpus.saturating_sub(1) {
                if lines.len() >= room {
                    break;
                }
                let name = format!("hog-{}", hogs.len() + extra + 1);
                lines.push(self.plan_line(&name, hog, seed));
            }
        }
        lines.truncate(room);
        lines
    }

    fn plan_line(&self, name: &str, index: usize, seed: u64) -> String {
        let burst_ms = match self.procs[index].behavior {
            Behavior::Hog => HOG_BURST_MS,
            _ => WORK_BURST_MS,
        };
        format!(
            "{name} program=work pages={WORK_PAGES} burst_ms={burst_ms} mix={} index={index} seed={seed}",
            self.name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::TrialPlan;

    fn number(line: &str, key: &str) -> u64 {
        line.split_whitespace()
            .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{key} in {line}"))
    }

    /// No workload process may be killed as a runaway during a trial: a hog's
    /// limit outlasts the longest trial the kernel accepts (2 x 5 pairs of 10 s,
    /// plus warm-up), and a process that blocks never computes near its limit.
    #[test]
    fn no_workload_process_can_be_killed_as_a_runaway() {
        let longest_trial_ms = TrialPlan {
            warmup_ns: 1_000 * MS,
            window_ns: 10_000 * MS,
            pairs: 5,
            tail_ns: 200 * MS,
        }
        .total_ns()
            / MS;
        for mix in MIXES {
            for line in mix.plan_lines(1, 4, 16) {
                let index = number(&line, "index") as usize;
                let burst_ms = number(&line, "burst_ms");
                match mix.procs[index].behavior {
                    Behavior::Hog => assert!(burst_ms > 10 * longest_trial_ms, "{line}"),
                    Behavior::Wakeful { run, .. } => assert!(run.high * 10 <= burst_ms * MS),
                    Behavior::Probe { burst, .. } => assert!(burst.high * 10 <= burst_ms * MS),
                }
            }
        }
    }

    #[test]
    fn plan_lines_name_the_mix_and_fit_the_room() {
        let lines = MIX_A.plan_lines(7, 1, 16);
        assert_eq!(lines.len(), MIX_A.procs.len());
        assert_eq!(
            lines[0],
            "editor program=work pages=128 burst_ms=2000 mix=A index=0 seed=7"
        );
        assert!(lines[6].starts_with("hog-1 program=work pages=128 burst_ms=3600000 "));
        let more = MIX_A.plan_lines(7, 3, 16);
        assert_eq!(more.len(), 10);
        assert!(more[9].starts_with("hog-4 ") && more[9].contains(" index=6 "));
        assert_eq!(MIX_A.plan_lines(7, 8, 9).len(), 9);
        assert_eq!(
            MIX_B.plan_lines(7, 2, 16)[7].split(' ').next(),
            Some("hog-7")
        );
    }
}
