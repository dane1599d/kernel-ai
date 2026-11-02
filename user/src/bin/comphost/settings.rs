//! The Settings app's `kai:settings` requests on their way to the shell (P4
//! task 7): what the shown version's `send` queued goes to `desktop`
//! (`deskcore::settings_wire`), which knows this process by the kernel's name
//! for it, and the answers come back through `replies.on-reply`, each a call
//! of its own like a window's input. What is owed to whom is
//! `wasmhost::settings_ledger`'s; this is the transport.
//!
//! Only an app the system image grants `kai:settings` reaches here: its plan
//! line is named [`SETTINGS_APP`], and `desktop` and the shell check that name
//! again.

use crate::started::shown_serial;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use deskcore::settings_wire::{self as wire, MAX_WAITING};
use kabi::handle::Handle;
use livecore::host::{Env, Host};
use user::{println, sys};
use wasmhost::LiveInstance;
use wasmhost::settings_ledger::{Ledger, Owed};

pub use deskcore::settings_wire::SETTINGS_APP;

#[derive(Default)]
pub struct Forward {
    ledger: Ledger,
    /// Whether an answer has reached the app yet, for the one line that says so.
    told: bool,
}

impl Forward {
    /// Whether a request waits for its answer: the app is in use, so it does
    /// not nap, and comphost keeps asking `desktop`.
    pub fn busy(&self) -> bool {
        self.ledger.busy()
    }

    /// Takes what the shown version sent, passes it on as room allows, and
    /// gives the app the answers that came; whether any was given.
    pub fn pump<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
        route: Handle,
        reply: &mut [u8],
    ) -> bool {
        let mut owed = self.take(host);
        owed.extend(self.send(route, reply));
        owed.extend(self.results(route, reply));
        let any = !owed.is_empty();
        let given = give(host, owed);
        if given && !self.told {
            self.told = true;
            println!("comphost: the first settings answer reached the app");
        }
        any
    }

    /// The shown version's requests; a standby's are dropped, so a rehearsed
    /// version never sends twice.
    fn take<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>) -> Vec<Owed> {
        let mut owed = Vec::new();
        host.each_instance(|instance, _, shown| {
            if let LiveInstance::V2(core) = instance {
                let taken = core.take_settings();
                if shown {
                    owed.extend(self.ledger.queue(core.serial(), taken));
                }
            }
        });
        owed
    }

    /// Puts as many held requests as `desktop` has room for; ones it refuses,
    /// or that cannot reach it, are owed why.
    fn send(&mut self, route: Handle, reply: &mut [u8]) -> Vec<Owed> {
        let room = MAX_WAITING.saturating_sub(self.ledger.in_flight());
        let out = self.ledger.outgoing(room);
        if out.is_empty() {
            return Vec::new();
        }
        let lines: Vec<String> = out
            .iter()
            .map(|o| {
                wire::encode_request(&wire::Request {
                    ticket: o.wire,
                    user: o.user,
                    text: o.text.clone(),
                })
            })
            .collect();
        let put = format!("{}{}", wire::PUT, lines.join("\n"));
        let why = match call(route, put.as_bytes(), reply) {
            Ok(text) if text == "ok" => return Vec::new(),
            Ok(text) => format!("desktop refused it: {text}"),
            Err(why) => why,
        };
        println!("comphost: settings requests not sent: {why}");
        let wires: Vec<u32> = out.iter().map(|o| o.wire).collect();
        self.ledger.refused(&wires, &why)
    }

    /// The answers `desktop` holds for the requests in flight; one for a
    /// ticket this process did not send is dropped.
    fn results(&mut self, route: Handle, reply: &mut [u8]) -> Vec<Owed> {
        if self.ledger.in_flight() == 0 {
            return Vec::new();
        }
        let text = match call(route, wire::RESULTS.as_bytes(), reply) {
            Ok(text) => text,
            Err(why) => {
                println!("comphost: settings answers not read: {why}");
                return Vec::new();
            }
        };
        let Some(answers) = wire::parse_answers(&text) else {
            println!("comphost: settings answers out of shape");
            return Vec::new();
        };
        let mut owed = Vec::new();
        for answer in answers {
            let outcome = answer.outcome.map(String::into_bytes);
            match self.ledger.answered(answer.ticket, outcome) {
                Some(found) => owed.push(found),
                None => println!("comphost: dropped a settings answer it did not ask for"),
            }
        }
        owed
    }
}

/// Gives each answer to the instance that asked, if it is still the one shown;
/// each is a transaction, like a window's input. One for an instance that is
/// gone is dropped: its successor never asked, and a suspended app is not
/// woken for it. Whether any reached the app.
fn give<E: Env<Instance = LiveInstance>>(host: &mut Host<E>, owed: Vec<Owed>) -> bool {
    let mut given = false;
    for answer in owed {
        let ticket = answer.ticket;
        if !answer.is_for(shown_serial(host)) {
            println!("comphost: dropped the settings answer to ticket {ticket}: its asker is gone");
            continue;
        }
        let outcome = answer.outcome;
        if crate::ui::serve_shown(host, |core| core.settings_reply(ticket, outcome)).is_none() {
            println!("comphost: the settings answer to ticket {ticket} did not reach the app");
        } else {
            given = true;
        }
    }
    given
}

/// One call to `desktop`, and its answer as text.
fn call(route: Handle, request: &[u8], reply: &mut [u8]) -> Result<String, String> {
    match sys::call_route(route, request, reply, crate::ui::CALL_MS) {
        Ok(len) => Ok(String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned()),
        Err(error) => Err(format!("desktop could not be asked ({error:?})")),
    }
}
