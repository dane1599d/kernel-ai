//! The `cc` service (ADR 0012 D2-D5; P4 task 12): the `c>` prompt's compiler.
//! Its logic is `ccserve`, tested on the host; this program binds it to the
//! system calls. Each request is one `kai-cc 1` message from the shell, and
//! each reply one message back.
//!
//! cc.wasm is translated, and kairt compiled, on the first request rather than
//! at boot: until someone types `:c`, cc only waits. It holds no authority
//! (`caps=serve`): a session reaches only its imports, and an effect goes back
//! to the shell as a `need`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec;
use ccserve::rows::Rows;
use ccserve::{Host, Service};
use kabi::Error;
use promptcore::wire::{Record, Reply};
use user::{println, sys};

user::entry!(main);

/// How much longer each compile and run may take when QEMU emulates the CPU
/// (TCG). cc runs about 25 times slower there than under KVM (cc.wasm's first
/// translation: 4.8 s against 0.2 s), so an entry over a 200-line session
/// passes D3's 2 s. At 12 s each, a compile and a run still answer within the
/// shell's 30 s wait for an entry. KVM and real hardware keep 2 s.
const TCG_STRETCH: u64 = 6;

struct Process {
    line_ns: u64,
}

impl Host for Process {
    fn now_ns(&self) -> u64 {
        sys::time_ns()
    }

    fn interrupted(&self) -> bool {
        sys::interrupted()
    }

    fn line_ns(&self) -> u64 {
        self.line_ns
    }
}

/// D3's line budget, stretched only when CPUID names QEMU's emulated CPU.
fn line_ns() -> u64 {
    match cpufeat::hypervisor_signature() {
        Some(signature) if &signature == b"TCGTCGTCGTCG" => ccserve::LINE_NS * TCG_STRETCH,
        _ => ccserve::LINE_NS,
    }
}

fn main() -> i32 {
    println!("up; cc.wasm loads on the first request");
    let host = Process { line_ns: line_ns() };
    if host.line_ns != ccserve::LINE_NS {
        println!(
            "QEMU emulates the CPU, so each compile and run gets {} ms",
            host.line_ns / 1_000_000
        );
    }
    let mut service: Option<Service<Rows>> = None;
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
        if service.is_none() {
            let start = sys::time_ns();
            match Service::new(ccserve::CC_WASM, &ccserve::XCC_HEADERS, Rows::os(), &host) {
                Ok(loaded) => {
                    let ms = (sys::time_ns() - start) / 1_000_000;
                    println!("cc.wasm translated and kairt compiled in {ms} ms");
                    service = Some(loaded);
                }
                Err(why) => {
                    println!("cannot start: {why}");
                    let mut reply = Reply::new(kabi::MAX_MESSAGE);
                    let text = format!("cc cannot start: {why}");
                    let _ = reply.push(&Record::Err {
                        line: 0,
                        text: text.as_bytes(),
                    });
                    let _ = sys::svc_reply(&reply.into_bytes());
                    continue;
                }
            }
        }
        let Some(service) = service.as_mut() else {
            continue;
        };
        let reply = service.handle(&request[..len], &host);
        if let Err(error) = sys::svc_reply(&reply) {
            println!("svc_reply failed: {error:?}");
        }
    }
}
