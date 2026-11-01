//! `deskdemo`, `desktop`'s test client (P3 plan §4 item 1): it opens two
//! windows through `kai-surface 1`, "Left" and "Right", fills each with one
//! colour, and closes a window when `desktop` asks it to. If `desktop` restarts,
//! the new one greets it as a new connection (the registry's globals) and it
//! opens its windows again from the pool it kept. Not an app, and never in the
//! pack.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use kabi::handle::{Handle, Rights};
use surfabi::proto::{Event, Interface, REGISTRY, Request};
use surfabi::{Format, Rect, wire};
use user::{println, sys};

user::entry!(main);

/// Each window's content, unless `size=WxH` on the plan line says another
/// (`desk-perf` drags an 800x600 one, as S05-1's scene (b) did), at most
/// [`MAX_SIDE`] a side.
const DEFAULT: Size = Size { w: 300, h: 200 };
const MAX_SIDE: u32 = 1600;
const CALL_MS: u64 = 5_000;
/// How often it asks `desktop` for its events.
const POLL_MS: u64 = 20;

/// (surface, toplevel, buffer, title, colour) for each window.
const WINDOWS: [(u32, u32, u32, &str, u32); 2] = [
    (3, 5, 4, "Left", 0xFF3A_6EA5),
    (10, 12, 11, "Right", 0xFF8A_4F2B),
];

#[derive(Clone, Copy)]
struct Size {
    w: u32,
    h: u32,
}

impl Size {
    fn from_args() -> Size {
        let args = user::args();
        let asked = user::arg(&args, "size").and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some(Size {
                w: w.parse().ok()?,
                h: h.parse().ok()?,
            })
        });
        let fits = |side: u32| (1..=MAX_SIDE).contains(&side);
        asked.filter(|s| fits(s.w) && fits(s.h)).unwrap_or(DEFAULT)
    }

    fn buffer(self) -> u32 {
        self.w * self.h * 4
    }

    fn pool(self) -> u32 {
        4096 + 2 * self.buffer()
    }
}

fn main() -> i32 {
    let Some(route) = user::route(0) else {
        println!("deskdemo: no route to desktop");
        return 1;
    };
    let size = Size::from_args();
    let pages = u64::from(size.pool()).div_ceil(4096);
    let Ok(shm) = sys::shm_create(pages) else {
        println!("deskdemo: no memory for its pool");
        return 1;
    };
    let Ok(address) = sys::shm_map(shm, true) else {
        println!("deskdemo: cannot map its pool");
        return 1;
    };
    // SAFETY: the pool is mapped read-write at `address`, `pages` pages long.
    let pool =
        unsafe { core::slice::from_raw_parts_mut(address as *mut u32, pages as usize * 1024) };
    for (n, window) in WINDOWS.iter().enumerate() {
        let start = (4096 + n as u32 * size.buffer()) as usize / 4;
        pool[start..start + (size.w * size.h) as usize].fill(window.4);
    }
    if !open(route, shm, size) {
        return 1;
    }
    println!("deskdemo: opened Left and Right");
    let mut open_now = [true; 2];
    let mut reply = alloc::vec![0u8; kabi::MAX_MESSAGE];
    loop {
        sys::sleep_ms(POLL_MS);
        let Ok(len) = sys::call_route(route, &[], &mut reply, CALL_MS) else {
            continue;
        };
        let mut closing = Vec::new();
        let mut restarted = false;
        let mut rest = &reply[..len.min(reply.len())];
        while let Ok((header, args, next)) = wire::split(rest) {
            rest = next;
            // Globals come only on a new connection: this is another desktop.
            if header.object == REGISTRY && header.opcode == 0 {
                restarted = true;
                continue;
            }
            let Some(n) = WINDOWS.iter().position(|w| w.1 == header.object) else {
                continue;
            };
            if Event::decode(Interface::Toplevel, header.opcode, args) == Ok(Event::Close)
                && open_now[n]
            {
                open_now[n] = false;
                closing.push(n);
            }
        }
        if restarted {
            if open(route, shm, size) {
                open_now = [true; 2];
                println!("deskdemo: reopened Left and Right after desktop restarted");
            }
            continue;
        }
        for n in closing {
            let destroy = Request::ToplevelDestroy
                .encode(WINDOWS[n].1)
                .unwrap_or_default();
            let _ = sys::call_route(route, &destroy, &mut reply, CALL_MS);
            println!("deskdemo: closed {}", WINDOWS[n].3);
            if open_now.iter().all(|o| !o) {
                println!("deskdemo: every window closed");
            }
        }
    }
}

/// Opens both windows on the pool `shm`, sending `desktop` a copy of the
/// handle so the pool stays this client's for another `desktop` later.
fn open(route: Handle, shm: Handle, size: Size) -> bool {
    let rights = Rights::MAP | Rights::READ | Rights::TRANSFER;
    let Ok(copy) = sys::handle_dup(shm, rights) else {
        println!("deskdemo: cannot copy its pool's handle");
        return false;
    };
    let mut requests = Vec::new();
    requests.push((
        REGISTRY,
        Request::Bind {
            name: 1,
            interface: "pool",
            version: 1,
            id: 2,
        },
    ));
    requests.push((2, Request::PoolCreate { size: size.pool() }));
    let (w, h) = (size.w, size.h);
    for (n, (surface, toplevel, buffer, title, _)) in WINDOWS.iter().enumerate() {
        let offset = 4096 + n as u32 * size.buffer();
        requests.extend([
            (
                REGISTRY,
                Request::Bind {
                    name: 2,
                    interface: "surface",
                    version: 1,
                    id: *surface,
                },
            ),
            (
                2,
                Request::PoolBuffer {
                    id: *buffer,
                    offset,
                    width: w,
                    height: h,
                    stride: w * 4,
                    format: Format::Xrgb8888,
                },
            ),
            (*surface, Request::GetToplevel { id: *toplevel }),
            (*toplevel, Request::SetTitle(title)),
            (*surface, Request::Attach { buffer: *buffer }),
            (*surface, Request::DamageBuffer(Rect { x: 0, y: 0, w, h })),
            (*surface, Request::Commit),
        ]);
    }
    let bytes: Vec<u8> = requests
        .iter()
        .flat_map(|(object, request)| request.encode(*object).unwrap_or_default())
        .collect();
    let mut reply = alloc::vec![0u8; kabi::MAX_MESSAGE];
    match sys::call_h(route, &bytes, &mut reply, CALL_MS, &[copy], &mut []) {
        Ok(_) => true,
        Err(error) => {
            println!("deskdemo: desktop refused its windows: {error:?}");
            false
        }
    }
}
