//! The edit service's view of the fake kernel: the store standing in for the
//! COMP_* calls, the build service over loopback standing in for BUILD.

use editcore::service::Env;
use editcore::wire;
use kabi::Error;
use kabi::live::Info;
use livecore::host::Host;
use livecore::local::{Kernel, LocalEnv};
use livecore::uploads;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
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
    ) -> Result<(), Error> {
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
        component
            .put(&meta, Arc::new(artifact.to_vec()), base, commit, 0)
            .map(|_| ())
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
        _component: &str,
        artifact: &[u8],
        base: &str,
        commit: &str,
    ) -> Result<(), Error> {
        self.upload(artifact, base, commit)
    }

    fn build(&mut self, request: &[u8], timeout_ms: u64) -> Result<Vec<u8>, Error> {
        let builds = request.starts_with(b"POST /build ");
        if builds {
            self.builds += 1;
        }
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).map_err(|_| Error::Network)?;
        stream
            .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
            .map_err(|_| Error::Network)?;
        stream.write_all(request).map_err(|_| Error::Network)?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|_| Error::TimedOut)?;
        if builds {
            self.last_build = Some(response.clone());
        }
        Ok(response)
    }

    fn control(&mut self, component: &str, request: &str) -> Result<Vec<u8>, Error> {
        relay(self.host.as_mut(), component, request)
    }

    fn log(&mut self, text: &str) {
        println!("  | [edit] {text}");
    }
}
