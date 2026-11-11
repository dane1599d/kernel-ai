//! `kai:net@0.1.0` for `lifecycle 2` (P5 tasks 12 and 13): HTTPS for an app,
//! through a granted interface. As with `kai:models`, `fetch` only queues;
//! comphost takes the queue after the call ([`CoreInstance::take_net`]), asks
//! the shell whether the app may reach the host, has the `fetch` service make
//! the request on a handle narrowed to that host, and gives the app the
//! answer in a later call, `responses.on-response`
//! ([`CoreInstance::net_response`]). So a call never waits on the network or
//! on the person's answer.
//!
//! A request waits in the instance's own short queue, so an answer only ever
//! reaches the instance that asked, and a reloaded, resumed or swapped one
//! starts with none. The interface is linked only for an instance loaded with
//! the `net` grant ([`Grants`]): its process holds a ceiling (`netdest=`) and
//! a route to `fetch`. Without it the import fails to link, and the version
//! does not stage (`Reason::Link`).

use crate::canonical::CoreInstance;
use crate::grants::Grants;
use crate::instance::HostState;
use crate::link::WitFunc;
use crate::settings::error_text;
use crate::tables;
use crate::ui::{InCall, core_of};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use appabi::Val;
use livecore::{Fault, Reason};
use wasmi::{Caller, Error};

/// The interface whose function this module serves.
pub(crate) const NET: &str = "kai:net/http";
/// The export the answers go to.
pub(crate) const RESPONSES: &str = "kai:net/responses";

/// Most requests waiting for the host; a call that makes more traps.
pub const MAX_QUEUED: usize = 4;
/// The longest URL kept: a longer one is answered `bad-url` by comphost.
pub const MAX_URL: usize = 2048;
/// The longest body the app is given (`kainet::fetch::MAX_BODY`).
pub const MAX_BODY: usize = 64 * 1024;

/// What the app asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetRequest {
    pub url: String,
    /// HEAD, not GET.
    pub head: bool,
    /// Bytes `first` to `last` inclusive, or to the end.
    pub range: Option<(u64, Option<u64>)>,
}

/// What the network answered, as `responses.response` holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetResponse {
    pub status: u16,
    pub url: String,
    pub content_type: Option<String>,
    pub retry_after: Option<u32>,
    pub body: Vec<u8>,
}

/// Why a request failed, as `responses.error` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    Refused(String),
    BadUrl(String),
    Tls(String),
    Http(String),
    TooLarge,
    Timeout,
    Truncated,
}

impl NetError {
    /// The variant's case and its text, cut and made printable.
    fn val(&self) -> Val {
        let text = |why: &str| Some(Box::new(Val::String(error_text(why))));
        match self {
            Self::Refused(why) => Val::Variant(0, text(why)),
            Self::BadUrl(why) => Val::Variant(1, text(why)),
            Self::Tls(why) => Val::Variant(2, text(why)),
            Self::Http(why) => Val::Variant(3, text(why)),
            Self::TooLarge => Val::Variant(4, None),
            Self::Timeout => Val::Variant(5, None),
            Self::Truncated => Val::Variant(6, None),
        }
    }
}

impl NetResponse {
    fn val(&self) -> Val {
        let text = |s: &str| Box::new(Val::String(String::from(s)));
        Val::Record(vec![
            Val::U16(self.status),
            Val::String(self.url.clone()),
            Val::Option(self.content_type.as_deref().map(text)),
            Val::Option(self.retry_after.map(|s| Box::new(Val::U32(s)))),
            Val::Bytes(self.body.clone()),
        ])
    }
}

/// The requests the app made and comphost has not taken, and the next ticket.
#[derive(Default)]
pub(crate) struct NetHost {
    queue: Vec<(u32, NetRequest)>,
    next: u32,
    /// Where this call's requests start in the queue.
    call_start: usize,
}

impl NetHost {
    fn push(&mut self, request: NetRequest) -> Result<u32, Error> {
        if self.queue.len() >= MAX_QUEUED {
            return Err(Error::new(format!(
                "{NET} fetch: {MAX_QUEUED} requests waiting for comphost"
            )));
        }
        let ticket = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| Error::new(format!("{NET} fetch: tickets used up")))?;
        self.queue.push((ticket, request));
        Ok(ticket)
    }
}

/// The request in `fetch`'s lifted argument.
fn request_of(args: &[Val]) -> Result<NetRequest, &'static str> {
    let [Val::Record(fields)] = args else {
        return Err("expected one request");
    };
    let [Val::String(url), Val::Enum(method), Val::Option(range)] = fields.as_slice() else {
        return Err("the request is out of shape");
    };
    let head = match method {
        0 => false,
        1 => true,
        _ => return Err("no such method"),
    };
    let range = match range.as_deref() {
        None => None,
        Some(Val::Record(bounds)) => match bounds.as_slice() {
            [Val::U64(first), Val::Option(last)] => {
                let last = match last.as_deref() {
                    None => None,
                    Some(Val::U64(last)) => Some(*last),
                    Some(_) => return Err("the range is out of shape"),
                };
                Some((*first, last))
            }
            _ => return Err("the range is out of shape"),
        },
        Some(_) => return Err("the range is out of shape"),
    };
    // A longer URL is kept empty: comphost answers it `bad-url`, and the
    // queue never holds more than its bound in URLs.
    let url = match url.len() <= MAX_URL {
        true => url.clone(),
        false => String::new(),
    };
    Ok(NetRequest { url, head, range })
}

/// `http.fetch(req) -> u32`: the request's ticket.
pub(crate) fn fetch(
    mut caller: Caller<'_, HostState>,
    entry: &'static WitFunc,
    params: &[wasmi::Val],
    results: &mut [wasmi::Val],
) -> Result<(), Error> {
    let core = params.iter().map(core_of).collect::<Result<Vec<_>, _>>()?;
    let func = entry.func(String::new());
    let mut guest = InCall {
        caller: &mut caller,
    };
    let (args, _) = appabi::lift_args(&mut guest, &func, &core).map_err(|t| Error::new(t.0))?;
    let request = request_of(&args).map_err(|why| Error::new(format!("{NET} fetch: {why}")))?;
    let ticket = caller.data_mut().net.push(request)?;
    let [result] = results else {
        return Err(Error::new(format!("{NET} fetch: one result expected")));
    };
    *result = wasmi::Val::I32(ticket as i32);
    Ok(())
}

/// Whether `grants` lets an app import `kai:net/http`.
pub(crate) fn granted(grants: Grants) -> bool {
    grants.net
}

impl CoreInstance {
    /// Starts counting a call's requests: where they start in the queue.
    pub(crate) fn net_call_begins(&mut self) {
        let net = &mut self.store.data_mut().net;
        net.call_start = net.queue.len();
    }

    /// A call faulted: what it queued is not sent.
    pub(crate) fn net_call_faulted(&mut self) {
        let net = &mut self.store.data_mut().net;
        let start = net.call_start.min(net.queue.len());
        net.queue.truncate(start);
    }

    /// Whether a request waits to be taken.
    pub fn net_waiting(&self) -> bool {
        !self.store.data().net.queue.is_empty()
    }

    /// The requests the app made since the last call here, oldest first, with
    /// their tickets.
    pub fn take_net(&mut self) -> Vec<(u32, NetRequest)> {
        core::mem::take(&mut self.store.data_mut().net.queue)
    }

    /// Tells the app what the request with `ticket` came to. A body over
    /// [`MAX_BODY`] is `too-large`, and an error's text is cut and made
    /// printable. An app that does not export `kai:net/responses` is told
    /// nothing.
    pub fn net_response(
        &mut self,
        ticket: u32,
        outcome: Result<NetResponse, NetError>,
    ) -> Result<(), Fault> {
        let Some(export) = self.on_net_response.clone() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == RESPONSES && f.name == "on-response")
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:net/responses table"))?;
        let outcome = match outcome {
            Ok(response) if response.body.len() > MAX_BODY => Err(NetError::TooLarge),
            other => other,
        };
        let outcome = match outcome {
            Ok(response) => Val::Result(Ok(Some(Box::new(response.val())))),
            Err(error) => Val::Result(Err(Some(Box::new(error.val())))),
        };
        livecore::Instance::call(self, &entry.func(export), &[Val::U32(ticket), outcome])?;
        Ok(())
    }
}
