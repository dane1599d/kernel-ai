//! The arms a run compares: what each one feeds the model, parsed from `--arms`.

use super::render::Format;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Tagged(Format),
    /// Today's script path; `no_think` false is arm A0.
    M8,
    /// The OS rows' calls on llm's own `act` path ([`os`]).
    Os,
    /// View-then-act on llm's own `act` path ([`view`]).
    View,
    /// The shipped apps' rows on llm's own `act` path ([`os::APP_TASKS`]).
    Apps,
    /// The 190 tasks on the shipped pack, report only ([`shipped`]).
    Shipped,
    /// Spike S13-5's multi-step goals on llm's own `act` path ([`goals`]).
    Goals,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Arm {
    pub(super) kind: Kind,
    pub(super) rows: usize,
    pub(super) no_think: bool,
    /// `+rules`: the M8 prompt's Rhai rules as the card.
    pub(super) rules: bool,
    /// `+m8script`: a `script` call's code comes from today's script prompt
    /// and envelope (the plan's fallback for M8-style requests).
    pub(super) m8script: bool,
    /// `act`: what ships. The 190 tasks as `dsl-1+m8script` with the shipping
    /// decoder and consent; the M8 50 through the shell's one door, with llm's
    /// own `act` path (`llmproto::local::act::routed`) on the OS pack.
    pub(super) ship: bool,
    /// `+index`: rows no task needs (the 80-row scale's distractors) are
    /// named on index lines only, and close with no arguments (ADR 0010 §6).
    pub(super) index: bool,
}

impl Arm {
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        match text {
            "m8" => {
                return Ok(Self {
                    kind: Kind::M8,
                    rows: 40,
                    no_think: true,
                    rules: false,
                    m8script: false,
                    ship: false,
                    index: false,
                });
            }
            "act" => {
                return Ok(Self {
                    kind: Kind::Tagged(Format::Dsl1),
                    rows: 40,
                    no_think: false,
                    rules: false,
                    m8script: true,
                    ship: true,
                    index: false,
                });
            }
            "shipped" => {
                return Ok(Self {
                    kind: Kind::Shipped,
                    rows: 0,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: true,
                    index: false,
                });
            }
            "os" => {
                return Ok(Self {
                    kind: Kind::Os,
                    rows: 0,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: true,
                    index: false,
                });
            }
            "apps" => {
                return Ok(Self {
                    kind: Kind::Apps,
                    rows: 0,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: true,
                    index: false,
                });
            }
            "goals" => {
                return Ok(Self {
                    kind: Kind::Goals,
                    rows: 0,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: true,
                    index: false,
                });
            }
            "view" => {
                return Ok(Self {
                    kind: Kind::View,
                    rows: 0,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: true,
                    index: false,
                });
            }
            "m8-a0" => {
                return Ok(Self {
                    kind: Kind::M8,
                    rows: 40,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: false,
                    index: false,
                });
            }
            _ => {}
        }
        let (text, rows) = match text.split_once('@') {
            Some((arm, "80")) => (arm, 80),
            Some((arm, "40")) => (arm, 40),
            Some(_) => return Err(format!("{text}: the scale is @40 or @80")),
            None => (text, 40),
        };
        let (text, no_think) = match text.strip_suffix(":no-think") {
            Some(arm) => (arm, true),
            None => (text, false),
        };
        let (mut text, mut rules, mut m8script, mut index) = (text, false, false, false);
        loop {
            if let Some(arm) = text.strip_suffix("+rules") {
                (text, rules) = (arm, true);
            } else if let Some(arm) = text.strip_suffix("+m8script") {
                (text, m8script) = (arm, true);
            } else if let Some(arm) = text.strip_suffix("+index") {
                (text, index) = (arm, true);
            } else {
                break;
            }
        }
        let format = Format::parse(text).ok_or(format!(
            "unknown arm {text}: dsl-1, hermes-1, prose-1, m8 or m8-a0"
        ))?;
        Ok(Self {
            kind: Kind::Tagged(format),
            rows,
            no_think,
            rules,
            m8script,
            ship: false,
            index,
        })
    }

    pub(super) fn label(&self) -> String {
        match self.kind {
            Kind::M8 if self.no_think => "m8".into(),
            Kind::M8 => "m8-a0".into(),
            Kind::Os => "os".into(),
            Kind::Shipped => "shipped".into(),
            Kind::View => "view".into(),
            Kind::Goals => "goals".into(),
            Kind::Apps => "apps".into(),
            Kind::Tagged(_) if self.ship => "act".into(),
            Kind::Tagged(format) => format!(
                "{}{}{}{}{}@{}",
                format.name(),
                if self.rules { "+rules" } else { "" },
                if self.m8script { "+m8script" } else { "" },
                if self.index { "+index" } else { "" },
                if self.no_think { ":no-think" } else { "" },
                self.rows
            ),
        }
    }
}
