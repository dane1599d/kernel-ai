//! Who made a key, a click or a line: the mark that decides whose words count.
//!
//! The person's consent comes only from what the person did: a line typed on the
//! keyboard, a key pressed for a question. The compositor marks such input `user`;
//! an agent's `ui_act` keys, text it types or sends, and anything a script makes carry
//! no such mark. A [`Mark`] is that fact as a value, and it is opaque: the only way to
//! a person's mark is [`Source::hardware`], and an agent's input can only be
//! [`Source::agent`], so no call turns an agent's input into the person's.
//!
//! The compiler cannot check where an input came from, so [`Source::hardware`] is an
//! `unsafe fn`: a claim that the input is the person's own, made by the code that took
//! it from the kernel's input path or the compositor's device events. Every place that
//! makes the claim is `unsafe { Source::hardware() }` and shows up in a search and a
//! review; nothing an agent can reach has the right to make it.

use crate::roster::AgentId;

/// Where a key, a click or a line came from, as the code that took it knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source(Origin);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Hardware,
    Agent(AgentId),
}

impl Source {
    /// The person's own keyboard or pointer.
    ///
    /// # Safety
    ///
    /// Not memory safety: the caller claims the input came from the person's
    /// hardware, as the kernel's input path or the compositor's own device events took
    /// it. An input an agent made (`ui_act`'s keys, text it typed or sent, a script's
    /// output, a copy of what the person once said) must never be claimed.
    pub unsafe fn hardware() -> Self {
        Self(Origin::Hardware)
    }

    /// Something agent `id` made.
    pub const fn agent(id: AgentId) -> Self {
        Self(Origin::Agent(id))
    }
}

/// What an input is marked as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark(Who);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Person,
    Agent(AgentId),
}

impl Mark {
    /// The mark of input from `source`.
    pub const fn of(source: Source) -> Self {
        Self(match source.0 {
            Origin::Hardware => Who::Person,
            Origin::Agent(id) => Who::Agent(id),
        })
    }

    /// Whether a person made it: the `user` flag on a key.
    pub const fn is_person(self) -> bool {
        matches!(self.0, Who::Person)
    }

    /// The agent that made it, if one did.
    pub const fn agent(self) -> Option<AgentId> {
        match self.0 {
            Who::Agent(id) => Some(id),
            Who::Person => None,
        }
    }
}

/// A line of text and where it came from. Its mark is its source's, and nothing
/// else can set it.
#[derive(Clone, Copy, Debug)]
pub struct Line<'a> {
    text: &'a str,
    mark: Mark,
}

impl<'a> Line<'a> {
    pub const fn new(text: &'a str, source: Source) -> Self {
        Self {
            text,
            mark: Mark::of(source),
        }
    }

    pub const fn text(&self) -> &'a str {
        self.text
    }

    pub const fn mark(&self) -> Mark {
        self.mark
    }
}
