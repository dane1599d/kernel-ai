//! `deskdemo`, `desktop`'s test client (P3 plan §4 item 1): it opens two
//! windows through `kai-surface 1`, "Left" and "Right", fills each with one
//! colour, and closes a window when `desktop` asks it to. Not an app, and never
//! in the pack.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use surfabi::proto::{Event, Interface, REGISTRY, Request};
use surfabi::{Format, Rect, wire};
use user::{println, sys};

user::entry!(main);

const W: u32 = 300;
const H: u32 = 200;
const BUFFER: u32 = W * H * 4;
const POOL: u32 = 4096 + 2 * BUFFER;
const CALL_MS: u64 = 5_000;
/// How often it asks `desktop` for its events.
const POLL_MS: u64 = 20;

/// (surface, toplevel, buffer, title, colour) for each window.
const WINDOWS: [(u32, u32, u32, &str, u32); 2] = [
    (3, 5, 4, "Left", 0xFF3A_6EA5),
    (10, 12, 11, "Right", 0xFF8A_4F2B),
];

fn main() -> i32 {
    let Some(route) = user::route(0) else {
        println!("deskdemo: no route to desktop");
        return 1;
    };
    let pages = u64::from(POOL).div_ceil(4096);
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
    requests.push((2, Request::PoolCreate { size: POOL }));
    for (n, (surface, toplevel, buffer, title, colour)) in WINDOWS.iter().enumerate() {
        let offset = 4096 + n as u32 * BUFFER;
        let start = offset as usize / 4;
        pool[start..start + (W * H) as usize].fill(*colour);
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
                    width: W,
                    height: H,
                    stride: W * 4,
                    format: Format::Xrgb8888,
                },
            ),
            (*surface, Request::GetToplevel { id: *toplevel }),
            (*toplevel, Request::SetTitle(title)),
            (*surface, Request::Attach { buffer: *buffer }),
            (
                *surface,
                Request::DamageBuffer(Rect {
                    x: 0,
                    y: 0,
                    w: W,
                    h: H,
                }),
            ),
            (*surface, Request::Commit),
        ]);
    }
    let bytes: Vec<u8> = requests
        .iter()
        .flat_map(|(object, request)| request.encode(*object).unwrap_or_default())
        .collect();
    let mut reply = alloc::vec![0u8; kabi::MAX_MESSAGE];
    if let Err(error) = sys::call_h(route, &bytes, &mut reply, CALL_MS, &[shm], &mut []) {
        println!("deskdemo: desktop refused its windows: {error:?}");
        return 1;
    }
    println!("deskdemo: opened Left and Right");
    let mut open = [true; 2];
    while open.iter().any(|o| *o) {
        sys::sleep_ms(POLL_MS);
        let Ok(len) = sys::call_route(route, &[], &mut reply, CALL_MS) else {
            continue;
        };
        let mut closing = Vec::new();
        let mut rest = &reply[..len.min(reply.len())];
        while let Ok((header, args, next)) = wire::split(rest) {
            rest = next;
            let Some(n) = WINDOWS.iter().position(|w| w.1 == header.object) else {
                continue;
            };
            if Event::decode(Interface::Toplevel, header.opcode, args) == Ok(Event::Close)
                && open[n]
            {
                open[n] = false;
                closing.push(n);
            }
        }
        for n in closing {
            let destroy = Request::ToplevelDestroy
                .encode(WINDOWS[n].1)
                .unwrap_or_default();
            let _ = sys::call_route(route, &destroy, &mut reply, CALL_MS);
            println!("deskdemo: closed {}", WINDOWS[n].3);
        }
    }
    println!("deskdemo: every window closed");
    loop {
        sys::sleep_ms(60_000);
    }
}
