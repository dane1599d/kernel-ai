//! Spike S05-1 "compose and look" (ADR 0005): renders the scenes on the real
//! display and times them with the TSC. Never merged.
//!
//! - `role=scenes`: maps the back buffer (SPIKE_DISPLAY_MAP), composes in its
//!   own RAM frame and copies only damaged rows to it, then flushes them
//!   (SPIKE_DISPLAY_FLUSH). Runs (a)-(d) and vello_cpu's (a) and (b), prints
//!   p50, p95 and p99 of render and flush, and at 1920x1080 shows each panel
//!   state in both themes for xtask's screendumps.
//! - `role=drag`: scene (b) again, for (f) while the kernel runs hogs.
//! - `role=compositor` and `role=client`: scene (e). The client redraws a
//!   600x400 region per key into a shared buffer and rings the compositor
//!   (SPIKE_BELL_RING); the compositor wakes (SPIKE_BELL_WAIT), composites it,
//!   flushes, and rings back. Key to flush is TSC from the client's key to the
//!   compositor's flush returning.
//!
//! Exit codes: 0 done, 1 no display, 2 no shared buffer.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spike_compose::desk::{Desk, PanelState};
use spike_compose::frame::{Frame, Rect};
use spike_compose::look::{DARK, LIGHT, Look};
use spike_compose::scenes::{self, Screen, Timings};
use spike_compose::stats::{Summary, summary};
use spike_compose::text::Text;
use user::{arg, args, println, sys};

user::entry!(main);

fn tsc() -> u64 {
    // SAFETY: RDTSC reads a counter and has no other effect; ring 3 may run it.
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn cycles_per_ms() -> u64 {
    let (ns, t) = (sys::time_ns(), tsc());
    sys::sleep_ms(50);
    let (ns, t) = (sys::time_ns() - ns, tsc() - t);
    (t.saturating_mul(1_000_000) / ns.max(1)).max(1)
}

/// The scanout buffer the kernel mapped, and where frames go.
struct Display {
    base: *mut u32,
    w: i32,
    h: i32,
    stride: usize,
}

impl Display {
    fn map() -> Option<Display> {
        let mut info = [0u64; 3];
        let base = sys::spike_display_map(&mut info).ok()?;
        Some(Display {
            base: base as *mut u32,
            w: info[0] as i32,
            h: info[1] as i32,
            stride: info[2] as usize,
        })
    }
}

impl Screen for Display {
    fn present(&mut self, frame: &Frame, damage: &[Rect]) {
        let mut rects: Vec<[u32; 4]> = Vec::new();
        for d in damage.iter().take(32) {
            let r = d.intersect(&frame.bounds());
            if r.is_empty() {
                continue;
            }
            for y in r.y..r.bottom() {
                let from = (y * frame.w + r.x) as usize;
                let src = &frame.px[from..from + r.w as usize];
                // SAFETY: the row lies in the mapped scanout buffer: `y` is below
                // its height and `r.x + r.w` within its width, the stride is its.
                unsafe {
                    let dst = self.base.add(y as usize * self.stride + r.x as usize);
                    core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
                }
            }
            rects.push([r.x as u32, r.y as u32, r.w as u32, r.h as u32]);
        }
        let _ = sys::spike_display_flush(&rects);
    }

    fn now(&self) -> u64 {
        tsc()
    }
}

fn line(name: &str, w: i32, h: i32, scale: f32, what: &str, s: Summary) {
    println!(
        "compose: ({name}) {w}x{h} scale {scale:.2} {what}: p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} ms over {}",
        s.p50, s.p95, s.p99, s.max, s.n
    );
}

fn report(name: &str, (w, h): (i32, i32), scale: f32, t: &Timings, per_ms: u64) {
    line(name, w, h, scale, "render", summary(&t.render, per_ms));
    line(name, w, h, scale, "flush", summary(&t.flush, per_ms));
}

fn setup(look: Look) -> Option<(Display, Desk, Text, f32)> {
    let display = Display::map()?;
    let scale = arg(&args(), "scale")
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| scenes::default_scale(display.w));
    let mut text = Text::new(spike_compose::INTER, spike_compose::MONO, look.dark);
    text.hint = scale == 1.0;
    let desk = scenes::desk(display.w, display.h, look, scale, &mut text);
    Some((display, desk, text, scale))
}

fn run_scenes() -> i32 {
    let per_ms = cycles_per_ms();
    let Some((mut screen, mut d, mut text, scale)) = setup(DARK) else {
        return 1;
    };
    println!(
        "compose: {} TSC cycles a ms; display {}x{} stride {}",
        per_ms, screen.w, screen.h, screen.stride
    );
    let all = d.frame.bounds();
    d.compose(all);
    screen.present(&d.frame, &[all]);
    let size = (d.frame.w, d.frame.h);
    report("a", size, scale, &scenes::full(&mut d, &mut screen), per_ms);
    report("b", size, scale, &scenes::drag(&mut d, &mut screen), per_ms);
    let (cold, warm) = scenes::text_page(&mut d, &mut text, &mut screen);
    report("c cold", size, scale, &cold, per_ms);
    report("c warm", size, scale, &warm, per_ms);
    report(
        "d",
        size,
        scale,
        &scenes::sheet(&mut d, &mut text, &mut screen),
        per_ms,
    );
    let (vf, vd) = scenes::vello(&mut d, &mut screen);
    line(
        "a vello",
        d.frame.w,
        d.frame.h,
        scale,
        "render",
        summary(&vf, per_ms),
    );
    line(
        "b vello",
        d.frame.w,
        d.frame.h,
        scale,
        "render",
        summary(&vd, per_ms),
    );
    // The screendumps: at 1920x1080 only, the size the founder looks at.
    if screen.w == 1920 && screen.h == 1080 {
        for look in [DARK, LIGHT] {
            let mut text = Text::new(spike_compose::INTER, spike_compose::MONO, look.dark);
            text.hint = scale == 1.0;
            let mut d = scenes::desk(screen.w, screen.h, look, scale, &mut text);
            for state in PanelState::ALL {
                d.set_panel(&mut text, state);
                d.compose(all);
                screen.present(&d.frame, &[all]);
                let theme = if look.dark { "dark" } else { "light" };
                println!("compose: screendump {theme}-{}", state.name());
                let _ = sys::spike_mark();
                sys::sleep_ms(2500);
            }
        }
    }
    0
}

fn run_drag() -> i32 {
    let per_ms = cycles_per_ms();
    let Some((mut screen, mut d, _, scale)) = setup(DARK) else {
        return 1;
    };
    let size = (d.frame.w, d.frame.h);
    report("f", size, scale, &scenes::drag(&mut d, &mut screen), per_ms);
    0
}

/// Scene (e)'s shared buffer: a header page, then the client's 600x400 pixels.
#[repr(C)]
struct Header {
    /// Bumped by the client for each key; the compositor waits on it.
    key: AtomicU32,
    /// Set by the compositor to the key it flushed; the client waits on it.
    done: AtomicU32,
    /// The TSC at the client's key.
    at: AtomicU64,
    /// The client's last key, then u32::MAX to stop.
    last: AtomicU32,
}

const E_W: i32 = 600;
const E_H: i32 = 400;
const KEYS: u32 = 300;

fn shared() -> Option<(&'static Header, *mut u32)> {
    let bytes = 4096 + (E_W * E_H * 4) as u64;
    let base = sys::spike_share_map(0, bytes).ok()?;
    // SAFETY: the kernel mapped `bytes` of zeroed shared pages at `base`; the
    // header's atomics are what both processes use them through.
    let header = unsafe { &*(base as *const Header) };
    Some((header, (base + 4096) as *mut u32))
}

fn run_client() -> i32 {
    let Some((header, pixels)) = shared() else {
        return 2;
    };
    let mut text = Text::new(spike_compose::INTER, spike_compose::MONO, true);
    let mut f = Frame::new(E_W, E_H);
    for n in 1..=KEYS {
        let at = tsc();
        scenes::client_draw(&mut f, &mut text, DARK, 1.0, n);
        // SAFETY: the shared pixels are E_W * E_H u32s after the header page.
        unsafe { core::ptr::copy_nonoverlapping(f.px.as_ptr(), pixels, f.px.len()) };
        header.at.store(at, Ordering::SeqCst);
        header.key.store(n, Ordering::SeqCst);
        let _ = sys::spike_bell_ring(&header.key);
        while header.done.load(Ordering::SeqCst) != n {
            let _ = sys::spike_bell_wait(&header.done, n - 1, 1000);
        }
        // Keys come at most every 2 ms, as a fast typist's would not.
        sys::sleep_ms(2);
    }
    header.last.store(u32::MAX, Ordering::SeqCst);
    header.key.store(KEYS + 1, Ordering::SeqCst);
    let _ = sys::spike_bell_ring(&header.key);
    0
}

fn run_compositor() -> i32 {
    let per_ms = cycles_per_ms();
    let Some((mut screen, mut d, mut text, scale)) = setup(DARK) else {
        return 1;
    };
    let Some((header, pixels)) = shared() else {
        return 2;
    };
    let place = Rect::new(d.frame.w / 2 - E_W / 2, d.frame.h / 2 - E_H / 2, E_W, E_H);
    let bar = spike_compose::desk::px(32.0, scale);
    let frame = Rect::new(place.x - 1, place.y - bar, E_W + 2, E_H + bar + 1);
    d.add_window(&mut text, frame, "client", true, 255, &mut |_, _, _| {});
    let all = d.frame.bounds();
    d.compose(all);
    screen.present(&d.frame, &[all]);
    let mut samples = Vec::new();
    let mut seen = 0u32;
    loop {
        let _ = sys::spike_bell_wait(&header.key, seen, 5000);
        let key = header.key.load(Ordering::SeqCst);
        if key == seen {
            continue;
        }
        if header.last.load(Ordering::SeqCst) == u32::MAX {
            break;
        }
        seen = key;
        let i = d.windows.len() - 1;
        let (ox, oy) = (place.x - d.windows[i].rect.x, place.y - d.windows[i].rect.y);
        let layer = &mut d.windows[i].layer;
        for y in 0..E_H {
            let dst = ((oy + y) * layer.w + ox) as usize;
            // SAFETY: row `y` of the shared pixels, E_W u32s.
            let src = unsafe {
                core::slice::from_raw_parts(pixels.add((y * E_W) as usize), E_W as usize)
            };
            layer.px[dst..dst + E_W as usize].copy_from_slice(src);
        }
        d.compose(place);
        screen.present(&d.frame, &[place]);
        samples.push(tsc().saturating_sub(header.at.load(Ordering::SeqCst)));
        header.done.store(key, Ordering::SeqCst);
        let _ = sys::spike_bell_ring(&header.done);
    }
    line(
        "e",
        d.frame.w,
        d.frame.h,
        scale,
        "key to flush",
        summary(&samples, per_ms),
    );
    0
}

fn main() -> i32 {
    let args = args();
    match arg(&args, "role") {
        Some("scenes") => run_scenes(),
        Some("drag") => run_drag(),
        Some("client") => run_client(),
        Some("compositor") => run_compositor(),
        _ => {
            println!("compose: role=scenes|drag|client|compositor");
            1
        }
    }
}
