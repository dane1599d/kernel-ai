//! [`PolicySpec`]: the whole of what M10's policy agent may choose, as a small
//! `Copy` value that is validated before anything is built. A policy from the
//! catalog and one of two presets; no numbers. The presets' numbers live here and
//! are checked against [`classes`](crate::classes)' bounds in code, so a preset
//! cannot drift out of them.

use crate::classes::{ClassParams, Classes, MS, Responsive};
use crate::{Interactive, RoundRobin, SchedPolicy};
use alloc::boxed::Box;

/// The policy catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyKind {
    /// The default: woken threads first, preempt on wake.
    Interactive,
    /// FIFO with 10 ms slices; the watchdog's fallback.
    RoundRobin,
    /// Interactive with background labels and a preset.
    Classes,
    /// Interactive that boosts only short bursts; the rule-based baseline.
    Responsive,
}

impl PolicyKind {
    pub const ALL: [PolicyKind; 4] = [
        PolicyKind::Interactive,
        PolicyKind::RoundRobin,
        PolicyKind::Classes,
        PolicyKind::Responsive,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PolicyKind::Interactive => "interactive",
            PolicyKind::RoundRobin => "round-robin",
            PolicyKind::Classes => "classes",
            PolicyKind::Responsive => "responsive",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Whether background labels change anything under this policy.
    pub fn uses_labels(self) -> bool {
        self == PolicyKind::Classes
    }
}

/// How hard background threads are demoted under `classes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// A short maximum wait: background work keeps a steady trickle.
    Gentle,
    /// A longer maximum wait: the foreground gets more room.
    Firm,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::Gentle, Preset::Firm];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Gentle => "gentle",
            Preset::Firm => "firm",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.name() == name)
    }

    /// The normal ring's slice and the maximum wait, in nanoseconds.
    pub fn numbers(self) -> (u64, u64) {
        match self {
            Preset::Gentle => (6 * MS, 20 * MS),
            Preset::Firm => (4 * MS, 60 * MS),
        }
    }

    /// The preset's parameters, or `None` if its numbers left the bounds.
    pub fn params(self) -> Option<ClassParams> {
        let (slice, wait) = self.numbers();
        ClassParams::new(slice, wait)
    }
}

/// Why a spec was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    UnknownPolicy,
    UnknownPreset,
    /// A preset's numbers are outside the bounds (a bug caught before use).
    OutOfBounds,
}

impl SpecError {
    pub fn describe(self) -> &'static str {
        match self {
            SpecError::UnknownPolicy => "the policy is not in the catalog",
            SpecError::UnknownPreset => "the preset is not gentle or firm",
            SpecError::OutOfBounds => "the preset's numbers are outside their bounds",
        }
    }
}

/// A policy and a preset. The preset matters only for `classes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicySpec {
    pub policy: PolicyKind,
    pub preset: Preset,
}

impl PolicySpec {
    /// What the kernel runs until someone chooses otherwise.
    pub const DEFAULT: PolicySpec = PolicySpec {
        policy: PolicyKind::Interactive,
        preset: Preset::Gentle,
    };

    pub const fn new(policy: PolicyKind, preset: Preset) -> Self {
        Self { policy, preset }
    }

    pub fn parse(policy: &str, preset: &str) -> Result<Self, SpecError> {
        let spec = Self {
            policy: PolicyKind::from_name(policy).ok_or(SpecError::UnknownPolicy)?,
            preset: Preset::from_name(preset).ok_or(SpecError::UnknownPreset)?,
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<(), SpecError> {
        match self.policy {
            PolicyKind::Classes => self
                .preset
                .params()
                .map(|_| ())
                .ok_or(SpecError::OutOfBounds),
            _ => Ok(()),
        }
    }

    /// Builds the policy. Allocates, so never call it from interrupt context.
    pub fn build(&self) -> Result<Box<dyn SchedPolicy>, SpecError> {
        self.validate()?;
        Ok(match self.policy {
            PolicyKind::Interactive => Box::new(Interactive::new()),
            PolicyKind::RoundRobin => Box::new(RoundRobin::new()),
            PolicyKind::Responsive => Box::new(Responsive::new()),
            PolicyKind::Classes => Box::new(Classes::new(
                self.preset.params().ok_or(SpecError::OutOfBounds)?,
            )),
        })
    }

    /// `classes/firm`, or the policy's name alone when the preset does not matter.
    pub fn label(&self) -> LabelText {
        LabelText(*self)
    }
}

/// Display helper for [`PolicySpec::label`].
pub struct LabelText(PolicySpec);

impl core::fmt::Display for LabelText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let spec = self.0;
        match spec.policy {
            PolicyKind::Classes => write!(f, "{}/{}", spec.policy.name(), spec.preset.name()),
            other => f.write_str(other.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::{MAX_WAIT_BOUNDS_NS, NORMAL_SLICE_BOUNDS_NS};
    use alloc::format;

    #[test]
    fn every_preset_is_inside_its_bounds() {
        for preset in Preset::ALL {
            let (slice, wait) = preset.numbers();
            assert!((NORMAL_SLICE_BOUNDS_NS.0..=NORMAL_SLICE_BOUNDS_NS.1).contains(&slice));
            assert!((MAX_WAIT_BOUNDS_NS.0..=MAX_WAIT_BOUNDS_NS.1).contains(&wait));
            assert!(preset.params().is_some());
        }
        assert_eq!(Preset::Gentle.numbers().1, 20 * MS);
        assert_eq!(Preset::Firm.numbers().1, 60 * MS);
    }

    #[test]
    fn out_of_bounds_parameters_cannot_be_built() {
        assert!(ClassParams::new(3 * MS, 20 * MS).is_none());
        assert!(ClassParams::new(21 * MS, 20 * MS).is_none());
        assert!(ClassParams::new(6 * MS, 9 * MS).is_none());
        assert!(ClassParams::new(6 * MS, 81 * MS).is_none());
        assert!(ClassParams::new(4 * MS, 10 * MS).is_some());
        assert!(ClassParams::new(20 * MS, 80 * MS).is_some());
    }

    #[test]
    fn specs_parse_by_name_and_refuse_anything_else() {
        let spec = PolicySpec::parse("classes", "firm").unwrap();
        assert_eq!(spec, PolicySpec::new(PolicyKind::Classes, Preset::Firm));
        assert_eq!(format!("{}", spec.label()), "classes/firm");
        assert_eq!(format!("{}", PolicySpec::DEFAULT.label()), "interactive");
        assert_eq!(
            PolicySpec::parse("fifo", "firm"),
            Err(SpecError::UnknownPolicy)
        );
        assert_eq!(
            PolicySpec::parse("classes", "hard"),
            Err(SpecError::UnknownPreset)
        );
        assert_eq!(
            PolicySpec::parse("Classes", "firm"),
            Err(SpecError::UnknownPolicy)
        );
        for kind in PolicyKind::ALL {
            let spec = PolicySpec::new(kind, Preset::Gentle);
            assert_eq!(spec.build().unwrap().name(), kind.name());
        }
    }
}
