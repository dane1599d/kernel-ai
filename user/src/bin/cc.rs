//! The `cc` service (ADR 0012 D2-D5; P4 task 12): the `c>` prompt's compiler.
//! Its logic is `ccserve`, tested on the host; this program binds it to the
//! system calls. Each request is one `kai-cc 1` message from the shell, and
//! each reply one message back.
//!
//! cc.wasm is translated, and kairt compiled, on the first request rather than
//! at boot: until someone types `:c`, cc only waits. It holds no authority
//! (`caps=serve`): a session reaches only its imports, and an effect goes back
//! to the shell as a `need`. A `cc build` brings its sources in an `Shm`,
//! which cc maps for that request alone and writes the artifact into. A
//! `trial` brings the artifact in one, which cc maps read-only.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;
use ccserve::build::shm;
use ccserve::rows::Rows;
use ccserve::{Host, Service, Shm};
use kabi::Error;
use kabi::handle::{Handle, Kind, RecvInfo};
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
        let mut info = RecvInfo::default();
        let len = match sys::recv_h(&mut request, 0, &mut info) {
            Ok(len) => len.min(request.len()),
            Err(Error::TimedOut) => continue,
            Err(error) => {
                println!("recv_h failed: {error:?}");
                sys::sleep_ms(100);
                continue;
            }
        };
        let count = (info.handle_count as usize).min(info.handles.len());
        let moved: Vec<Handle> = info.handles[..count].iter().map(|&h| Handle(h)).collect();
        if service.is_none() {
            let start = sys::time_ns();
            match Service::new(ccserve::CC_WASM, &ccserve::XCC_HEADERS, Rows::os(), &host) {
                Ok(loaded) => {
                    let ms = (sys::time_ns() - start) / 1_000_000;
                    println!("cc.wasm translated and kairt compiled in {ms} ms");
                    service = Some(loaded);
                }
                Err(why) => println!("cannot start: {why}"),
            }
        }
        let reply = match service.as_mut() {
            Some(service) => answer(service, &request[..len], &moved, &host),
            None => refusal("cc cannot start"),
        };
        for handle in moved {
            let _ = sys::handle_close(handle);
        }
        if let Err(error) = sys::svc_reply(&reply) {
            println!("svc_reply failed: {error:?}");
        }
    }
}

/// `request`'s reply. A `build` brings one `Shm`, mapped read-write for it; a
/// `trial`'s, and any other request's, is mapped read-only
/// ([`ccserve::writes_shm`]).
fn answer(
    service: &mut Service<Rows>,
    request: &[u8],
    moved: &[Handle],
    host: &Process,
) -> Vec<u8> {
    let shm = match moved {
        [] => return service.handle(request, host),
        [shm] => *shm,
        _ => return refusal("a request brings one Shm at most"),
    };
    let is_shm = sys::handle_info(shm).is_ok_and(|info| info.kind() == Some(Kind::Shm));
    if !is_shm {
        return refusal("a request brings an Shm, not another kind");
    }
    let write = ccserve::writes_shm(request);
    let address = match sys::shm_map(shm, write) {
        Ok(address) => address,
        Err(error) => {
            let how = if write { "read-write" } else { "read-only" };
            return refusal(&format!("cc cannot map the Shm {how}: {error:?}"));
        }
    };
    // SAFETY: the kernel mapped the Shm at `address`, read-write only when
    // `write`, in a slot of its own (`kabi::layout::SHM_SLOT`, 1 GiB), and the
    // shell makes it `shm::BYTES` long. Past a shorter one's pages the slot is
    // unmapped, so it faults cc and reaches no other memory. The shell maps it
    // too, but it is blocked in `call_h` until this reply, and cc copies the
    // bundle or artifact out before it reads it all the same (`shm::serve`,
    // `trial::serve`). The slice is mutable only when the mapping is, and is
    // not used after the unmap.
    // TODO: refuse a short Shm once HANDLE_INFO reports an Shm's pages.
    let mapped = match write {
        true => {
            Shm::Write(unsafe { core::slice::from_raw_parts_mut(address as *mut u8, shm::BYTES) })
        }
        false => {
            Shm::Read(unsafe { core::slice::from_raw_parts(address as *const u8, shm::BYTES) })
        }
    };
    let reply = service.handle_with(request, Some(mapped), host);
    if let Err(error) = sys::shm_unmap(address) {
        println!("shm_unmap failed: {error:?}");
    }
    reply
}

/// An `err` reply for what stops cc before the service answers. It is never
/// the program's fault, so it starts `internal: ` (`ccserve::trial::INTERNAL`),
/// which the shell tells from the program's own errors.
fn refusal(text: &str) -> Vec<u8> {
    let text = format!("{}{text}", ccserve::trial::INTERNAL);
    let mut reply = Reply::new(kabi::MAX_MESSAGE);
    let _ = reply.push(&Record::Err {
        line: 0,
        text: text.as_bytes(),
    });
    reply.into_bytes()
}
