//! What the host owes the Settings app for its `kai:settings` requests on
//! their way to the shell, without the transport (comphost carries them through
//! `desktop`). The ticket on the wire is the host's own, kept with the app's
//! ticket and the instance's [`CoreInstance::serial`](crate::CoreInstance::serial):
//! an instance reloaded after a fault, swapped or resumed starts its tickets
//! again, so an answer is given only to the instance that asked.
//!
//! A change goes on only when a real key or click made it (`user`); any other
//! is owed its refusal at once, as the shell would answer it.

use crate::settings::{Kind, SettingsRequest};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// What a change not made by a key or click is answered with.
pub const NOT_BY_USER: &str = "a change needs your key or click";

/// A request ready to leave, under the host's own ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub wire: u32,
    pub user: bool,
    pub text: String,
}

/// An answer the host owes the instance `serial` for its `ticket`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owed {
    pub serial: u64,
    pub ticket: u32,
    pub outcome: Result<Vec<u8>, String>,
}

impl Owed {
    /// Whether it goes to the instance shown now; one for an instance that is
    /// gone is dropped, since its successor never asked.
    pub fn is_for(&self, shown: Option<u64>) -> bool {
        shown == Some(self.serial)
    }
}

struct Held {
    serial: u64,
    ticket: u32,
    user: bool,
    text: String,
}

struct Flight {
    wire: u32,
    serial: u64,
    ticket: u32,
}

#[derive(Default)]
pub struct Ledger {
    /// The next ticket on the wire.
    next: u32,
    held: VecDeque<Held>,
    flight: Vec<Flight>,
}

impl Ledger {
    /// Whether a request waits for its answer.
    pub fn busy(&self) -> bool {
        !self.held.is_empty() || !self.flight.is_empty()
    }

    /// Keeps what instance `serial` sent until there is room for it; a change
    /// no key or click made is owed its refusal now.
    pub fn queue(&mut self, serial: u64, requests: Vec<SettingsRequest>) -> Vec<Owed> {
        let mut refused = Vec::new();
        for request in requests {
            if request.kind == Kind::Change && !request.user {
                refused.push(Owed {
                    serial,
                    ticket: request.ticket,
                    outcome: Err(String::from(NOT_BY_USER)),
                });
                continue;
            }
            self.held.push_back(Held {
                serial,
                ticket: request.ticket,
                user: request.user,
                text: request.request,
            });
        }
        refused
    }

    /// Up to `room` held requests, oldest first, now in flight under tickets
    /// of the host's own.
    pub fn outgoing(&mut self, room: usize) -> Vec<Outgoing> {
        let count = room.min(self.held.len());
        let mut out = Vec::with_capacity(count);
        for held in self.held.drain(..count) {
            let wire = self.next;
            self.next = self.next.wrapping_add(1);
            self.flight.push(Flight {
                wire,
                serial: held.serial,
                ticket: held.ticket,
            });
            out.push(Outgoing {
                wire,
                user: held.user,
                text: held.text,
            });
        }
        out
    }

    /// Requests in flight.
    pub fn in_flight(&self) -> usize {
        self.flight.len()
    }

    /// `wires` never arrived: each is owed `why`.
    pub fn refused(&mut self, wires: &[u32], why: &str) -> Vec<Owed> {
        wires
            .iter()
            .filter_map(|&wire| self.answered(wire, Err(String::from(why))))
            .collect()
    }

    /// The answer to `wire`, for whoever asked; `None` for a ticket not in
    /// flight, which the host did not send or has answered already.
    pub fn answered(&mut self, wire: u32, outcome: Result<Vec<u8>, String>) -> Option<Owed> {
        let at = self.flight.iter().position(|f| f.wire == wire)?;
        let flight = self.flight.swap_remove(at);
        Some(Owed {
            serial: flight.serial,
            ticket: flight.ticket,
            outcome,
        })
    }
}
