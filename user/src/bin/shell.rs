//! The Rhai shell: answers each line the kernel terminal sends with `ok <text>` or
//! `err <text>`. The evaluation itself lives in the host-tested `shellcore` crate;
//! this program connects it to the system calls.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use shellcore::{DEFAULT_CONFIG, Host, Shell};
use user::{arg, args, println, sys};

user::entry!(main);

/// How long `ask` waits for the llm service.
const ASK_TIMEOUT_MS: u64 = 140_000;

struct System;

impl Host for System {
    fn now_ns(&self) -> u64 {
        sys::time_ns()
    }

    fn interrupted(&self) -> bool {
        sys::interrupted()
    }

    fn heap_bytes(&self) -> u64 {
        user::heap_bytes()
    }

    fn metrics(&self) -> Result<kabi::Metrics, kabi::Error> {
        sys::metrics()
    }

    fn set_policy(&self, id: u64) -> Result<(), kabi::Error> {
        sys::set_policy(id).map(|_| ())
    }

    fn ask(&self, prompt: &str) -> Result<String, String> {
        let request = alloc::format!("ask {prompt}");
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::service_call("llm", request.as_bytes(), &mut reply, ASK_TIMEOUT_MS)
            .map_err(|error| alloc::format!("llm service: {error:?}"))?;
        let text = String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned();
        match text.split_once(' ').unwrap_or((text.as_str(), "")) {
            ("ok", answer) => Ok(answer.to_string()),
            ("err", why) => Err(why.to_string()),
            _ => Err("llm service: malformed reply".to_string()),
        }
    }
}

fn main() -> i32 {
    let args = args();
    let mut shell = Shell::new(Rc::new(System), DEFAULT_CONFIG);
    println!("up (restart {})", arg(&args, "restart").unwrap_or("0"));
    let mut line = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let mut caller = u64::MAX;
        let len = match sys::svc_recv(&mut line, 0, &mut caller) {
            Ok(len) => len.min(line.len()),
            Err(kabi::Error::TimedOut) => continue,
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
                continue;
            }
        };
        // Only the kernel terminal may run shell lines.
        let mut reply = if caller == 0 {
            let text = core::str::from_utf8(&line[..len]).unwrap_or("");
            shell.eval(text).to_wire()
        } else {
            String::from("err the shell answers only the terminal")
        };
        reply.truncate(reply.floor_char_boundary(kabi::MAX_MESSAGE));
        if let Err(error) = sys::svc_reply(reply.as_bytes()) {
            println!("svc_reply failed: {error:?}");
        }
    }
}
