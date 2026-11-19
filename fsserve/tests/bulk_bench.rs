//! Host figures for `kai-fs 1`'s bulk path, to see where a 1 MiB piece's time
//! goes before the guest case (`fs-bench`) measures it: 32 MiB written in 1 MiB
//! `WriteShm`s inside one action and committed, then read back in 1 MiB
//! `ReadShm`s, through the `fs` binary's stack (kaifs's block cache over
//! `Batched` over a disk in RAM that counts its calls).
//!
//! Host CPU time only: the guest pays about 120 µs a disk call on top (batch.rs).
//! Release builds: `cargo test --release -p fsserve --test bulk_bench -- --ignored --nocapture`

use fsproto::{How, MAX_DATA, MAX_SHM_DATA, Reply, Request, decode_reply, encode_request};
use fsserve::batch::{Batched, RunDev};
use fsserve::{Call, DIR_RIGHTS, FILE_RIGHTS, Server};
use kabi::fs::HOME_USER;
use kaifs::cache::Cached;
use kaifs::{BLOCK, DevInfo, IoError};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

const FILE: usize = 32 << 20;
const DISK_BLOCKS: u64 = 64 * 1024;
const CACHE_BLOCKS: usize = 4096;
const CALLER: u64 = 7;

/// A disk in RAM that counts its calls and the blocks they move.
struct Ram {
    bytes: Vec<u8>,
    /// Calls and blocks moved, shared with the bench.
    count: Rc<Cell<(u64, u64)>>,
}

impl Ram {
    fn new(blocks: u64, count: Rc<Cell<(u64, u64)>>) -> Self {
        Ram {
            bytes: vec![0; blocks as usize * BLOCK],
            count,
        }
    }

    fn span(&mut self, addr: u64, len: usize) -> std::ops::Range<usize> {
        let (calls, blocks) = self.count.get();
        self.count.set((calls + 1, blocks + (len / BLOCK) as u64));
        let from = addr as usize * BLOCK;
        from..from + len
    }
}

impl RunDev for Ram {
    fn info(&self) -> DevInfo {
        DevInfo {
            blocks: (self.bytes.len() / BLOCK) as u64,
            volatile_cache: true,
            flush: true,
        }
    }

    fn read_run(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let span = self.span(addr, buf.len());
        buf.copy_from_slice(&self.bytes[span]);
        Ok(())
    }

    fn write_run(&mut self, addr: u64, data: &[u8]) -> Result<(), IoError> {
        let span = self.span(addr, data.len());
        self.bytes[span].copy_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

type Stack = Cached<Batched<Ram>>;

struct Bench {
    server: Server<Stack>,
    now: u64,
    count: Rc<Cell<(u64, u64)>>,
}

impl Bench {
    fn call(
        &mut self,
        badge: u64,
        rights: kabi::handle::Rights,
        request: Request,
        shm: Option<&mut [u8]>,
    ) -> (Reply, Option<u64>) {
        self.now += 1_000;
        let bytes = encode_request(&request);
        let answer = self.server.call_shm(
            &Call {
                caller: CALLER,
                badge,
                rights,
                request: &bytes,
                now: self.now,
            },
            shm,
        );
        (
            decode_reply(&answer.reply).unwrap(),
            answer.mint.map(|m| m.badge),
        )
    }

    fn home(&mut self, request: Request) -> (Reply, Option<u64>) {
        self.call(HOME_USER, DIR_RIGHTS, request, None)
    }

    fn ram(&self) -> (u64, u64) {
        self.count.get()
    }
}

fn mbs(bytes: usize, secs: f64) -> f64 {
    bytes as f64 / secs / 1e6
}

#[test]
#[ignore = "a figure, not a check: run it in release with --ignored --nocapture"]
fn bulk_32_mib_each_way() {
    let count = Rc::new(Cell::new((0, 0)));
    let disk = Cached::new(
        Batched::new(Ram::new(DISK_BLOCKS, count.clone())),
        CACHE_BLOCKS,
    );
    let (server, _) = Server::start(disk, [7; 16]).unwrap();
    let mut b = Bench {
        server,
        now: 0,
        count,
    };
    let (_, file) = b.home(Request::Open {
        path: "bench.bin".into(),
        rights: DIR_RIGHTS.bits(),
        how: How::CreateFile,
    });
    let file = file.expect("opened");
    let data: Vec<u8> = (0..FILE).map(|i| (i * 13 % 251) as u8).collect();
    let mut shm = vec![0u8; MAX_SHM_DATA];

    let before = b.ram();
    let start = Instant::now();
    b.home(Request::Begin { action: 1 });
    for (at, piece) in data.chunks(MAX_SHM_DATA).enumerate() {
        shm[..piece.len()].copy_from_slice(piece);
        let off = (at * MAX_SHM_DATA) as u64;
        let len = piece.len() as u32;
        let (r, _) = b.call(
            file,
            FILE_RIGHTS,
            Request::WriteShm { off, len },
            Some(&mut shm),
        );
        assert_eq!(r, Reply::Done);
    }
    let written = start.elapsed().as_secs_f64();
    let (r, _) = b.home(Request::Commit);
    assert!(matches!(r, Reply::Committed { .. }), "{r:?}");
    let write = start.elapsed().as_secs_f64();
    let after = b.ram();
    eprintln!(
        "bulk write: 32 MiB in 1 MiB pieces: {:.0} MB/s host ({:.1} ms pieces + {:.1} ms commit); {} disk calls, {} blocks",
        mbs(FILE, write),
        written * 1e3,
        (write - written) * 1e3,
        after.0 - before.0,
        after.1 - before.1
    );

    let before = b.ram();
    let start = Instant::now();
    let mut back = Vec::with_capacity(FILE);
    loop {
        let off = back.len() as u64;
        let len = MAX_SHM_DATA as u32;
        match b
            .call(
                file,
                FILE_RIGHTS,
                Request::ReadShm { off, len },
                Some(&mut shm),
            )
            .0
        {
            Reply::Copied { len: 0 } => break,
            Reply::Copied { len } => back.extend_from_slice(&shm[..len as usize]),
            other => panic!("{other:?}"),
        }
    }
    let read = start.elapsed().as_secs_f64();
    let after = b.ram();
    eprintln!(
        "bulk read: 32 MiB in 1 MiB pieces: {:.0} MB/s host ({:.1} ms); {} disk calls, {} blocks",
        mbs(FILE, read),
        read * 1e3,
        after.0 - before.0,
        after.1 - before.1
    );
    assert!(back == data);
}

#[test]
#[ignore = "a figure, not a check: run it in release with --ignored --nocapture"]
fn messages_32_mib_each_way() {
    let count = Rc::new(Cell::new((0, 0)));
    let disk = Cached::new(
        Batched::new(Ram::new(DISK_BLOCKS, count.clone())),
        CACHE_BLOCKS,
    );
    let (server, _) = Server::start(disk, [7; 16]).unwrap();
    let mut b = Bench {
        server,
        now: 0,
        count,
    };
    let (_, file) = b.home(Request::Open {
        path: "bench.bin".into(),
        rights: DIR_RIGHTS.bits(),
        how: How::CreateFile,
    });
    let file = file.expect("opened");
    let data: Vec<u8> = (0..FILE).map(|i| (i * 13 % 251) as u8).collect();
    let start = Instant::now();
    b.home(Request::Begin { action: 1 });
    for (at, piece) in data.chunks(MAX_DATA).enumerate() {
        let off = (at * MAX_DATA) as u64;
        let request = Request::Write {
            off,
            data: piece.to_vec(),
        };
        assert_eq!(b.call(file, FILE_RIGHTS, request, None).0, Reply::Done);
    }
    let (r, _) = b.home(Request::Commit);
    assert!(matches!(r, Reply::Committed { .. }), "{r:?}");
    let write = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let mut back = Vec::with_capacity(FILE);
    loop {
        let off = back.len() as u64;
        let len = MAX_DATA as u32;
        match b
            .call(file, FILE_RIGHTS, Request::Read { off, len }, None)
            .0
        {
            Reply::Data(d) if d.is_empty() => break,
            Reply::Data(d) => back.extend_from_slice(&d),
            other => panic!("{other:?}"),
        }
    }
    let read = start.elapsed().as_secs_f64();
    eprintln!(
        "messages: 32 MiB in {} {MAX_DATA} B messages: write {:.0} MB/s host, read {:.0} MB/s host",
        FILE.div_ceil(MAX_DATA),
        mbs(FILE, write),
        mbs(FILE, read)
    );
    assert!(back == data);
}
