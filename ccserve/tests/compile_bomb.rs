//! Bug-hunt-6 H6-8: a 4 KiB entry whose macros expand to hundreds of MiB
//! fills wcc's preprocessing tmpfile in `/tmp`. The compile's scratch tree is
//! capped at `compiler::COMPILE_SCRATCH`, so the compile fails while cc holds
//! no more than its budget counts; the old 32 MiB cap let the tmpfile alone
//! pass cc's whole plan.

use ccserve::{Budget, Compiler, Host};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The system allocator, counting live bytes and their peak.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout, passed on.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as allocated above.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

struct Clock;

impl Host for Clock {
    fn now_ns(&self) -> u64 {
        0
    }

    fn interrupted(&self) -> bool {
        false
    }
}

/// The hunt's entry: under 4 KiB, each `U` line about 360 KB preprocessed.
fn bomb() -> String {
    let mut entry = format!("#define S {}\n", "a".repeat(3600));
    entry += "#define T S S S S S S S S S S\n#define U T T T T T T T T T T\n";
    entry += "void Boom(void) {\n";
    entry += &"U\n".repeat(160);
    entry += "}\n";
    entry
}

#[test]
fn a_macro_bomb_fails_inside_the_compiles_room() {
    let entry = bomb();
    assert!(entry.len() <= 4096, "{}", entry.len());
    let compiler = Compiler::new(ccserve::CC_WASM, &ccserve::XCC_HEADERS).expect("cc.wasm");
    let mut budget = Budget::new(&Clock, u64::MAX);
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let compiled = compiler.compile(&entry, &mut budget);
    let held = PEAK.load(Ordering::Relaxed) - before;
    assert!(compiled.is_err(), "the bomb compiled");
    // What a compile may hold beyond the translated compiler: guest memory
    // and the scratch tree, with room for wasmi's own tables.
    let room = ccserve::compiler::MAX_MEMORY as u64 + ccserve::budget::COMPILE_TREE + (4 << 20);
    assert!(
        held as u64 <= room,
        "the compile held {} KiB, its room is {} KiB",
        held / 1024,
        room / 1024
    );
}
