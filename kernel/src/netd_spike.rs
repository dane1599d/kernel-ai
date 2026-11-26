//! Spike S07-4 (ADR 0007's gate for `netd`; G37), on `spike/netd-ring3` only:
//! the virtio network card driven from ring 3 against today's kernel path.
//!
//! - `netd=ring3` on the command line: the kernel finds the card and gives its
//!   first MSI-X entry a vector, as it always does, but brings up no stack.
//!   The `netd-spike` case then starts `netd` with three `Shm` handles, all
//!   kernel objects: the card's BAR (mapped uncached), a DMA arena below 4 GiB
//!   whose physical address is on its command line, and a KRNG v1 ring that
//!   `netbench`, the client, holds too. `netd` waits for the card's interrupt
//!   with NETD_WAIT, which sleeps on the `net` thread's wake (net::wake).
//! - Without it, the kernel's stack runs as on main, and `netbench` downloads
//!   through NET_CONNECT and NET_RECV, as a ring-3 program does today.
//!
//! The case measures the CPU the whole system spent, every CPU but its idle
//! thread, from the client's start to its exit, and logs one line the
//! workflow reads.

use crate::process::{self, SpawnRequest};
use crate::{hpet, log, memory, net, shm, threads};
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;
use captable::Entry;
use kabi::Error;
use kabi::handle::Kind;
use kabi::{ExitReason, Limits};
use mm::PAGE_SIZE;
use spin::Mutex;

/// NETD_WAIT(timeout_ms): sleeps until the card's MSI-X vector fires, or
/// `timeout_ms` passes; then interrupts from there on ask for another wait.
pub const NETD_WAIT: u64 = 240;

/// The DMA arena `netd` gets: its queues and every frame buffer (8 MiB).
const DMA_PAGES: u64 = 2048;
/// The KRNG ring between `netd` and `netbench`: 256 entries and 1 MiB of data.
pub const RING_ENTRIES: u32 = 256;
const RING_DATA: usize = 1 << 20;
/// How long the client may take, the transfer included.
const EXIT_TIMEOUT_MS: u64 = 600_000;

const LIMITS: Limits = Limits {
    max_pages: 16_384,
    max_burst_ms: 60_000,
    caps: 0,
    stack_pages: 0,
};

/// Where the card's virtio structures are: its BAR and their offsets in it.
#[derive(Clone, Copy, Debug)]
pub struct Card {
    pub base: u64,
    pub size: u64,
    pub common: u64,
    pub notify: u64,
    pub multiplier: u32,
    pub isr: u64,
    pub device: u64,
}

static CARD: Mutex<Option<Card>> = Mutex::new(None);

pub fn ring3(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|w| w == "netd=ring3")
}

/// Keeps the card for `netd`.
pub fn take(card: Option<Card>, vector: Option<u8>) {
    match (card, vector) {
        (Some(card), Some(vector)) => {
            log!("netd-spike: the card is netd's: {card:x?}, vector {vector}");
            *CARD.lock() = Some(card);
        }
        (card, vector) => log!("netd-spike: no card for netd ({card:?}, {vector:?})"),
    }
}

/// NETD_WAIT, or `None` for any other number.
pub fn handle(_slot: usize, number: u64, args: &[u64; 6]) -> Option<Result<u64, Error>> {
    if number != NETD_WAIT {
        return None;
    }
    net::wake::net_thread_sleep(args[0].clamp(1, 1000));
    net::wake::net_thread_polling();
    Some(Ok(net::wake::taken()))
}

/// `test=netd-spike mode=kernel|ring3 blob=A.B.C.D:PORT bytes=N`.
pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let word = |key: &str| cmdline.split_whitespace().find_map(|w| w.strip_prefix(key));
    let blob = word("blob=").ok_or("netd-spike: no blob=")?;
    let (server, port) = net::parse_endpoint(blob).ok_or("netd-spike: a bad blob=")?;
    let bytes: u64 = word("bytes=")
        .and_then(|n| n.parse().ok())
        .ok_or("netd-spike: no bytes=")?;
    let ring = ring3(cmdline);
    let mode = if ring { "ring3" } else { "kernel" };
    let ip = server.octets();
    let args = format!(
        "mode={mode} server={}.{}.{}.{}:{port} bytes={bytes}",
        ip[0], ip[1], ip[2], ip[3]
    );
    let (client_handles, netd) = if ring {
        let card = CARD.lock().ok_or("netd-spike: netd=ring3, but no card")?;
        let (bar, dma, dma_phys, ring) = objects(card)?;
        let netd_args = format!(
            "{args} dma={dma_phys:#x} dma_pages={DMA_PAGES} common={:#x} notify={:#x} mult={} isr={:#x} device={:#x}",
            card.common, card.notify, card.multiplier, card.isr, card.device
        );
        let netd = spawn("netd", &netd_args, vec![bar, dma, ring])?;
        (vec![ring], Some(netd))
    } else {
        wait_up()?;
        (vec![netdest(server, port)?], None)
    };
    let mut before = crate::sched_control::measure::Snapshots::new();
    let mut after = crate::sched_control::measure::Snapshots::new();
    before.take();
    let start = hpet::nanos();
    let client = spawn("netbench", &args, client_handles)?;
    let exit = wait_exit(client);
    let wall_ns = hpet::nanos() - start;
    after.take();
    if let Some(netd) = netd {
        let _ = process::kill(netd, ExitReason::Killed);
    }
    match exit? {
        (ExitReason::Exited, 0) => {}
        _ => return Err("netd-spike: netbench did not finish its transfer"),
    }
    let (_, idle_ns) = after.since(&before);
    let window = after.cores.first().map_or(0, |s| s.at) - before.cores.first().map_or(0, |s| s.at);
    let cpus = after.cores.len() as u64;
    let busy_ns = (window * cpus).saturating_sub(idle_ns);
    // CPU ms per GiB moved, over the client's whole life.
    let per_gib = busy_ns as u128 * (1 << 30) / u128::from(bytes.max(1)) / 1_000_000;
    log!(
        "netd-spike: mode={mode} bytes={bytes} wall_ms={} busy_ms={} cpus={cpus} cpu_ms_per_gib={per_gib} interrupts={}",
        wall_ns / 1_000_000,
        busy_ns / 1_000_000,
        net::wake::taken()
    );
    Ok(())
}

/// The card's BAR, the DMA arena (and its physical address) and the ring, each
/// a kernel `Shm` object's handle entry.
fn objects(card: Card) -> Result<(Entry, Entry, u64, Entry), &'static str> {
    let entry = |object| Entry::new(Kind::Shm, shm::created_rights(), object);
    let bar_pages = card.size.div_ceil(PAGE_SIZE);
    let bar = shm::create_device_object(card.base, bar_pages)
        .map_err(|_| "netd-spike: no object for the card's BAR")?;
    let dma_phys = contiguous(DMA_PAGES)?;
    let dma = shm::create_kernel_object(dma_phys, DMA_PAGES)
        .map_err(|_| "netd-spike: no object for the DMA arena")?;
    let ring_pages = (kabi::ring::size(RING_ENTRIES, RING_DATA) as u64).div_ceil(PAGE_SIZE);
    let ring_phys = contiguous(ring_pages)?;
    let ring = shm::create_kernel_object(ring_phys, ring_pages)
        .map_err(|_| "netd-spike: no object for the ring")?;
    Ok((entry(bar), entry(dma), dma_phys, entry(ring)))
}

/// `pages` zeroed contiguous frames below 4 GiB.
fn contiguous(pages: u64) -> Result<u64, &'static str> {
    let phys = memory::with(|m| m.allocate_contiguous_below(1 << 32, pages))
        .ok_or("netd-spike: no contiguous memory")?;
    // SAFETY: the frames were just allocated, and the direct map covers them.
    unsafe {
        core::ptr::write_bytes(
            (memory::hhdm() + phys) as *mut u8,
            0,
            (pages * PAGE_SIZE) as usize,
        )
    };
    Ok(phys)
}

/// A `NetDest` for the blob server, as `net_tests` makes them.
fn netdest(server: core::net::Ipv4Addr, port: u16) -> Result<Entry, &'static str> {
    let spec = format!("proto:tcp,addrs:{server},ports:{port},hosts:none");
    let dest = netdest::Dest::parse(&spec).map_err(|_| "netd-spike: a bad destination")?;
    let live = |object| crate::handle::names(Kind::NetDest, object);
    let object = net::dest::create(dest, live).ok_or("netd-spike: no room for a NetDest")?;
    Ok(Entry::new(
        Kind::NetDest,
        crate::handle::NETDEST_RIGHTS,
        object,
    ))
}

fn spawn(program: &str, args: &str, moved: Vec<Entry>) -> Result<u64, &'static str> {
    let request = SpawnRequest {
        name: program,
        program,
        args: args.as_bytes(),
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    process::spawn_with(&request, moved, false).map_err(|_| "netd-spike: cannot start a process")
}

/// The kernel's stack has its static address.
fn wait_up() -> Result<(), &'static str> {
    for _ in 0..1500 {
        if net::status().is_some_and(|s| s.address.is_some()) {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    Err("netd-spike: the network was not set up in time")
}

fn wait_exit(pid: u64) -> Result<(ExitReason, u64), &'static str> {
    for _ in 0..EXIT_TIMEOUT_MS / 10 {
        if let Some(exit) = process::supervision::kernel_child_exit(pid) {
            return Ok(exit);
        }
        threads::sleep_ms(10);
    }
    Err("netd-spike: netbench did not end in time")
}
