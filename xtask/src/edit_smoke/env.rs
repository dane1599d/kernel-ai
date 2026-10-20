//! The edit service's view of the fake kernel: the store standing in for the
//! COMP_* calls, the build service over loopback standing in for BUILD and
//! the kernel's background build job.

use editcore::service::Env;
use editcore::wire;
use kabi::Error;
use kabi::live::Info;
use livecore::host::Host;
use livecore::local::{Kernel, LocalEnv};
use livecore::uploads;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasmhost::WasmInstance;

/// The edit service's view of the fake kernel.
pub(super) struct StoreEnv {
    pub(super) kernel: Kernel,
    pub(super) port: u16,
    pub(super) builds: u64,
    /// The reply to the latest build, which the next upload must match, as the
    /// kernel requires.
    pub(super) last_build: Option<Vec<u8>>,
    /// The notes host, lent while the edit service handles a message, so a
    /// relayed control request reaches it.
    pub(super) host: Option<NotesHost>,
    /// Uploads stored as (component, base, label), which the kernel's COMP_PUT
    /// hook tells the journal.
    pub(super) uploaded: Vec<(String, String, String)>,
    /// The background build, as the kernel's build job holds it.
    pub(super) job: Option<Job>,
}

/// What an exchange with the build service came to.
type Exchanged = Result<Vec<u8>, Error>;

/// A build on a thread of its own: its result once the exchange ends.
pub(super) struct Job {
    result: Arc<Mutex<Option<Exchanged>>>,
    /// The reply was kept for the upload.
    kept: bool,
}

/// One request to the build service over loopback; a connection closed with
/// nothing sent back is no reply, as in the kernel.
fn exchange(port: u16, request: &[u8], timeout_ms: u64) -> Result<Vec<u8>, Error> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|_| Error::Network)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
        .map_err(|_| Error::Network)?;
    stream.write_all(request).map_err(|_| Error::Network)?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|_| Error::TimedOut)?;
    if response.is_empty() {
        return Err(Error::Network);
    }
    Ok(response)
}

pub(super) type NotesHost = Host<LocalEnv<WasmInstance>>;

/// What the kernel's COMP_CONTROL does: its own check of the request, then
/// `ctl <request>` to the notes host as the kernel.
pub(super) fn relay(
    host: Option<&mut NotesHost>,
    component: &str,
    request: &str,
) -> Result<Vec<u8>, Error> {
    if !kabi::live::relayable(request) {
        return Err(Error::Invalid);
    }
    let host = host
        .filter(|_| component == "notes")
        .ok_or(Error::NotFound)?;
    let request = format!("{}{request}", kabi::live::CONTROL_PREFIX);
    host.handle(0, request.as_bytes()).map_err(|_| Error::Dead)
}

impl StoreEnv {
    /// What the kernel's COMP_PUT does: the upload must be the latest build's
    /// artifact, then pass the store's rules.
    pub(super) fn upload(
        &mut self,
        artifact: &[u8],
        base: &str,
        commit: &str,
    ) -> Result<String, Error> {
        let reply = self.last_build.take();
        let bound = reply
            .as_deref()
            .map_or(Err("no build came before it"), |reply| {
                wire::check_built_response(reply, artifact, base, commit)
            });
        let mut component = self.kernel.component.borrow_mut();
        if bound.is_err() {
            component.uploads.rejected += 1;
            return Err(Error::Invalid);
        }
        let meta = uploads::meta(artifact);
        component.put(&meta, Arc::new(artifact.to_vec()), base, commit, 0)
    }
}

impl Env for StoreEnv {
    fn info(&mut self, component: &str) -> Result<Info, Error> {
        let store = self.kernel.component.borrow();
        if store.name != component {
            return Err(Error::NotFound);
        }
        Ok(store.info())
    }

    fn artifact(&mut self, _component: &str, label: &str) -> Result<Vec<u8>, Error> {
        let component = self.kernel.component.borrow();
        let label = match label {
            "" => component.ledger.active().ok_or(Error::NotFound)?,
            label => label,
        };
        Ok(component.bytes(label)?.as_slice().to_vec())
    }

    fn put(
        &mut self,
        component: &str,
        artifact: &[u8],
        base: &str,
        commit: &str,
    ) -> Result<(), Error> {
        let label = self.upload(artifact, base, commit)?;
        let stored = (component.to_string(), base.to_string(), label);
        self.uploaded.push(stored);
        Ok(())
    }

    fn build(&mut self, request: &[u8], timeout_ms: u64) -> Result<Vec<u8>, Error> {
        if !wire::is_sync_request(request) {
            return Err(Error::Invalid);
        }
        exchange(self.port, request, timeout_ms)
    }

    fn build_start(&mut self, request: &[u8], timeout_ms: u64) -> Result<(), Error> {
        if !wire::is_build_request(request) {
            return Err(Error::Invalid);
        }
        if self.job.is_some() {
            return Err(Error::Busy);
        }
        self.builds += 1;
        let result = Arc::new(Mutex::new(None));
        let (port, request, slot) = (self.port, request.to_vec(), result.clone());
        std::thread::spawn(move || {
            let reply = exchange(port, &request, timeout_ms);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(reply);
        });
        self.job = Some(Job {
            result,
            kept: false,
        });
        Ok(())
    }

    fn build_poll(&mut self, take: bool) -> Result<Vec<u8>, Error> {
        let job = self.job.as_mut().ok_or(Error::NotFound)?;
        let done = job.result.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match done {
            None => Err(Error::Busy),
            Some(Err(error)) => {
                self.job = None;
                Err(error)
            }
            Some(Ok(reply)) => {
                // As the kernel keeps the reply for COMP_PUT once the build ends.
                if !job.kept {
                    job.kept = true;
                    self.last_build = Some(reply.clone());
                }
                if !take {
                    return Ok(Vec::new());
                }
                self.job = None;
                Ok(reply)
            }
        }
    }

    fn control(&mut self, component: &str, request: &str) -> Result<Vec<u8>, Error> {
        relay(self.host.as_mut(), component, request)
    }

    fn log(&mut self, text: &str) {
        println!("  | [edit] {text}");
    }
}
