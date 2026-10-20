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
