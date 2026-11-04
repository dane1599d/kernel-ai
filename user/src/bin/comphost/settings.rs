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
//!
//! The Models window's switches (`kai:models` `use-model`, #556) travel the
//! same way on their own [`Channel`]: the same ledger, which answers a switch
//! no key or click made with why not, under `deskcore::models_wire`'s words.
//! Only the app the image grants `kai:models` carries them.

use crate::started::shown_serial;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use deskcore::settings_wire::{self as wire, MAX_WAITING, Words};
use kabi::handle::Handle;
use livecore::Fault;
use livecore::host::{Env, Host};
use user::{println, sys};
use wasmhost::settings_ledger::{Ledger, Owed};
use wasmhost::{CoreInstance, LiveInstance, SettingsRequest};

pub use deskcore::settings_wire::SETTINGS_APP;

/// Whose changes a [`Forward`] carries: the Settings app's requests, or the
/// Models window's switches. They differ only in what the host queued them in,
/// their words to `desktop` and the export their answers go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Settings,
    Models,
}

impl Channel {
    fn words(self) -> &'static Words {
        match self {
            Self::Settings => &wire::SETTINGS,
            Self::Models => &deskcore::models_wire::MODELS,
        }
    }

    /// What the log calls its requests.
    fn what(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Models => "model switch",
        }
    }

    fn take(self, core: &mut CoreInstance) -> Vec<SettingsRequest> {
        match self {
            Self::Settings => core.take_settings(),
            Self::Models => core.take_model_uses(),
        }
    }

    fn reply(
        self,
        core: &mut CoreInstance,
        ticket: u32,
        outcome: Result<Vec<u8>, String>,
    ) -> Result<(), Fault> {
        match self {
            Self::Settings => core.settings_reply(ticket, outcome),
            Self::Models => {
                let words = outcome.map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                core.models_used(ticket, words)
            }
        }
    }
}

pub struct Forward {
    channel: Channel,
    ledger: Ledger,
    /// Whether an answer has reached the app yet, for the one line that says so.
    told: bool,
}

impl Forward {
    pub fn new(channel: Channel) -> Self {
        Self {
            channel,
            ledger: Ledger::default(),
            told: false,
        }
    }

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
        let given = give(host, self.channel, owed);
        if given && !self.told {
            self.told = true;
            println!(
                "comphost: the first {} answer reached the app",
                self.channel.what()
            );
        }
        any
    }

    /// The shown version's requests; a standby's are dropped, so a rehearsed
    /// version never sends twice.
    fn take<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>) -> Vec<Owed> {
        let mut owed = Vec::new();
        let channel = self.channel;
        host.each_instance(|instance, _, shown| {
            if let LiveInstance::V2(core) = instance {
                let taken = channel.take(core);
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
        let put = format!("{}{}", self.channel.words().put, lines.join("\n"));
        let why = match call(route, put.as_bytes(), reply) {
            Ok(text) if text == "ok" => return Vec::new(),
            Ok(text) => format!("desktop refused it: {text}"),
            Err(why) => why,
        };
        println!("comphost: {} requests not sent: {why}", self.channel.what());
        let wires: Vec<u32> = out.iter().map(|o| o.wire).collect();
        self.ledger.refused(&wires, &why)
    }

    /// The answers `desktop` holds for the requests in flight; one for a
    /// ticket this process did not send is dropped.
    fn results(&mut self, route: Handle, reply: &mut [u8]) -> Vec<Owed> {
        if self.ledger.in_flight() == 0 {
            return Vec::new();
        }
        let what = self.channel.what();
        let text = match call(route, self.channel.words().results.as_bytes(), reply) {
            Ok(text) => text,
            Err(why) => {
                println!("comphost: {what} answers not read: {why}");
                return Vec::new();
            }
        };
        let Some(answers) = wire::parse_answers(&text) else {
            println!("comphost: {what} answers out of shape");
            return Vec::new();
        };
        let mut owed = Vec::new();
        for answer in answers {
            let outcome = answer.outcome.map(String::into_bytes);
            match self.ledger.answered(answer.ticket, outcome) {
                Some(found) => owed.push(found),
                None => println!("comphost: dropped a {what} answer it did not ask for"),
            }
        }
        owed
    }
}

/// Gives each answer to the instance that asked, if it is still the one shown;
/// each is a transaction, like a window's input. One for an instance that is
/// gone is dropped: its successor never asked, and a suspended app is not
/// woken for it. Whether any reached the app.
fn give<E: Env<Instance = LiveInstance>>(
    host: &mut Host<E>,
    channel: Channel,
    owed: Vec<Owed>,
) -> bool {
    let what = channel.what();
    let mut given = false;
    for answer in owed {
        let ticket = answer.ticket;
        if !answer.is_for(shown_serial(host)) {
            println!("comphost: dropped the {what} answer to ticket {ticket}: its asker is gone");
            continue;
        }
        let outcome = answer.outcome;
        if crate::ui::serve_shown(host, |core| channel.reply(core, ticket, outcome)).is_none() {
            println!("comphost: the {what} answer to ticket {ticket} did not reach the app");
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
