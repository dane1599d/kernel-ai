//! The edit service (M9b): the editing session behind the terminal's edit mode. It
//! reads versions from the kernel's store, sends edited source to the build service
//! the kernel was booted with, checks what comes back and uploads it. Its logic is
//! `editcore::service`, tested on the host; this program only binds it to the
//! system calls. It answers only the kernel, and only the kernel may call it.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use editcore::service::{Env, Service};
use kabi::live::{Info, MAX_UPLOAD};
use kabi::{Error, HttpRequest};
use llmproto::http;
use user::{arg, args, println, sys};

user::entry!(main);

struct Sys {
    response: Vec<u8>,
}

impl Env for Sys {
    fn info(&mut self, component: &str) -> Result<Info, Error> {
        sys::comp_read_info(component)
    }

    fn artifact(&mut self, component: &str, label: &str) -> Result<Vec<u8>, Error> {
        let mut buf = vec![0u8; MAX_UPLOAD];
        let mut len = sys::comp_read_artifact(component, label, &mut buf)?;
        if len > buf.len() {
            // A boot version can be larger than an upload.
            buf = vec![0u8; len];
            len = sys::comp_read_artifact(component, label, &mut buf)?;
        }
        buf.truncate(len);
        Ok(buf)
    }

    fn put(
        &mut self,
        component: &str,
        artifact: &[u8],
        base: &str,
        commit: &str,
    ) -> Result<(), Error> {
        sys::comp_put(component, artifact, base, commit)
    }

    fn build(&mut self, request: &[u8], timeout_ms: u64) -> Result<Vec<u8>, Error> {
        let call = HttpRequest {
            request_ptr: request.as_ptr() as u64,
            request_len: request.len() as u64,
            response_ptr: self.response.as_mut_ptr() as u64,
            response_cap: self.response.len() as u64,
            timeout_ms,
        };
        let len = sys::build(&call)?;
        Ok(self.response[..len.min(self.response.len())].to_vec())
    }

    fn control(&mut self, component: &str, request: &str) -> Result<Vec<u8>, Error> {
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::comp_control(component, request, &mut reply)?;
        reply.truncate(len);
        Ok(reply)
    }

    fn log(&mut self, text: &str) {
        println!("{text}");
    }
}

fn main() -> i32 {
    let args = args();
    let (Some(host), Some(token)) = (arg(&args, "host"), arg(&args, "token")) else {
        println!("no build service configured; exiting");
        return 1;
    };
    println!(
        "up (restart {}); build service {host}",
        arg(&args, "restart").unwrap_or("0")
    );
    let mut service = Service::new(host, token);
    let mut env = Sys {
        response: vec![0u8; http::MAX_RESPONSE],
    };
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let mut caller = 0;
        let len = match sys::svc_recv(&mut request, 0, &mut caller) {
            Ok(len) => len.min(request.len()),
            Err(Error::TimedOut) => continue,
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
                continue;
            }
        };
        let reply = service.handle(&mut env, caller, &request[..len]);
        if let Err(error) = sys::svc_reply(reply.as_bytes()) {
            println!("svc_reply failed: {error:?}");
        }
    }
}
