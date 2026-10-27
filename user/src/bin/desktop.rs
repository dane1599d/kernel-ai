//! `desktop`, for now only its kernel half's first client (P3 task 2): it claims
//! the screen, paints it one colour, and reports the input it gets. Ctrl+Alt+F12
//! takes the screen from it (a `Screen` event, its mapping now a zero page); it
//! unmaps, waits, and claims again when the second press gives the screen back.
//! Task 6 replaces this with the compositor (`deskcore`, ADR 0005 §1).

#![no_std]
#![no_main]

use kabi::display::{DisplayInfo, FlushRect};
use kabi::input::{InputEvent, Kind};
use user::{println, sys};

user::entry!(main);

/// Ember Workbench's raised surface: not the console's ink (`desk`), so a
/// screendump tells them apart.
const BACKGROUND: u32 = look::DARK.raised;

fn main() -> i32 {
    loop {
        let mut info = DisplayInfo::default();
        let shm = match sys::display_claim(&mut info) {
            Ok(shm) => shm,
            Err(error) => {
                println!("desktop: cannot claim the screen: {error:?}");
                if !wait_for_screen() {
                    return 1;
                }
                continue;
            }
        };
        let Ok(address) = sys::shm_map(shm, true) else {
            println!("desktop: cannot map the screen");
            return 1;
        };
        paint(address, &info);
        println!(
            "desktop: claimed {}x{} and painted it",
            info.width, info.height
        );
        let lost = serve();
        let _ = sys::shm_unmap(address);
        if !lost {
            return 1;
        }
        println!("desktop: lost the screen to the console");
        if !wait_for_screen() {
            return 1;
        }
        println!("desktop: the screen is back");
    }
}

fn paint(address: u64, info: &DisplayInfo) {
    let words = (info.stride / 4) as usize;
    // SAFETY: the kernel mapped `pages` pages at `address`, read-write.
    let pixels =
        unsafe { core::slice::from_raw_parts_mut(address as *mut u32, info.pages as usize * 1024) };
    for row in pixels.chunks_mut(words).take(info.height as usize) {
        row[..info.width as usize].fill(BACKGROUND);
    }
    let whole = FlushRect {
        x: 0,
        y: 0,
        w: info.width,
        h: info.height,
    };
    if let Err(error) = sys::display_flush(&[whole]) {
        println!("desktop: flush failed: {error:?}");
    }
}

/// Reports input until the screen is taken: `true` then, `false` on an error.
fn serve() -> bool {
    let mut events = [InputEvent {
        time_ns: 0,
        kind: Kind::Screen,
        code: 0,
        value: 0,
    }; 32];
    loop {
        let Ok(n) = sys::input_read(&mut events, 1000) else {
            return false;
        };
        for event in &events[..n] {
            match event.kind {
                Kind::Screen if event.value == 0 => return true,
                Kind::Key => println!("desktop: key {:#x} {}", event.code, event.value),
                Kind::Button => println!("desktop: button {} {}", event.code, event.value),
                _ => {}
            }
        }
    }
}

/// Waits for the screen to come back: `false` on an error.
fn wait_for_screen() -> bool {
    let mut events = [InputEvent {
        time_ns: 0,
        kind: Kind::Screen,
        code: 0,
        value: 0,
    }; 8];
    loop {
        let Ok(n) = sys::input_read(&mut events, 1000) else {
            return false;
        };
        if events[..n]
            .iter()
            .any(|e| e.kind == Kind::Screen && e.value == 1)
        {
            return true;
        }
    }
}
