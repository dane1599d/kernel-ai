//! KRNG v1 on the host: the layout, wrap-around, full and empty, a producer
//! and a consumer racing on two threads, and a hostile peer's indices.

extern crate std;

use super::*;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::NonNull;
use std::sync::Arc;
use std::thread;

/// An 8-byte-aligned region of `bytes`, kept alive by its owner.
struct Region {
    words: Vec<u64>,
}

impl Region {
    fn new(bytes: usize) -> Self {
        Self {
            words: vec![0; bytes.div_ceil(8)],
        }
    }

    fn ptr(&mut self) -> NonNull<u8> {
        NonNull::new(self.words.as_mut_ptr().cast()).unwrap()
    }

    fn len(&self) -> usize {
        self.words.len() * 8
    }

    fn header(&mut self) -> *mut Header {
        self.ptr().as_ptr().cast()
    }
}

fn request(tag: u64) -> Request {
    Request {
        tag,
        op: block::READ,
        len: 512,
        ..Request::default()
    }
}

#[test]
fn the_layout_is_the_adrs() {
    assert_eq!(size(8, 0), 64 + 8 * 64 + 8 * 16);
    let mut region = Region::new(size(8, 4096));
    let ring = unsafe { Ring::init(region.ptr(), region.len(), MSG, 8) }.unwrap();
    assert_eq!((ring.kind(), ring.entries()), (MSG, 8));
    assert_eq!(ring.data_area(), (size(8, 0), region.len() - size(8, 0)));
    let header = unsafe { &*region.header() };
    assert_eq!(
        (header.magic, header.version, header.entries),
        (MAGIC, 1, 8)
    );
    assert_eq!(header.rest, [0; 28]);
    // The kinds and ops, claimed in order.
    assert_eq!([BLOCK, FRAME, MSG, PCM, VMEXIT], [1, 2, 3, 4, 5]);
    assert_eq!([block::READ, block::WRITE, block::FLUSH], [1, 2, 3]);
    assert_eq!(
        [
            vmexit::MMIO,
            vmexit::PIO,
            vmexit::DOORBELL,
            vmexit::SHUTDOWN,
            vmexit::TRIPLE_FAULT
        ],
        [1, 2, 3, 4, 5]
    );
}

#[test]
fn a_ring_out_of_shape_is_refused() {
    let mut region = Region::new(size(16, 0));
    let (ptr, len) = (region.ptr(), region.len());
    for bad in [0, 4, 12, 8192] {
        assert_eq!(
            unsafe { Ring::init(ptr, len, BLOCK, bad) }.err(),
            Some(Error::Entries)
        );
    }
    assert_eq!(
        unsafe { Ring::init(ptr, size(16, 0) - 1, BLOCK, 16) }.err(),
        Some(Error::Size)
    );
    unsafe { Ring::init(ptr, len, BLOCK, 16) }.unwrap();
    unsafe { (*region.header()).version = 2 };
    assert_eq!(
        unsafe { Ring::attach(ptr, len) }.err(),
        Some(Error::Version)
    );
    unsafe { (*region.header()).magic = *b"KRNX" };
    assert_eq!(unsafe { Ring::attach(ptr, len) }.err(), Some(Error::Magic));
    unsafe {
        (*region.header()).magic = MAGIC;
        (*region.header()).version = 1;
        (*region.header()).entries = 32;
    }
    assert_eq!(
        unsafe { Ring::attach(ptr, len) }.err(),
        Some(Error::Size),
        "too small for 32"
    );
    let odd = NonNull::new(unsafe { ptr.as_ptr().add(4) }).unwrap();
    assert_eq!(
        unsafe { Ring::attach(odd, len - 4) }.err(),
        Some(Error::Size)
    );
}

#[test]
fn a_ring_fills_empties_and_wraps_around() {
    let mut region = Region::new(size(8, 0));
    let ring = unsafe { Ring::init(region.ptr(), region.len(), BLOCK, 8) }.unwrap();
    let mut tx = ring.request_producer();
    let mut rx = ring.request_consumer();
    assert_eq!(rx.pop(), Ok(None), "empty");
    let mut next = 0;
    let mut want = 0;
    // Three rounds and a half: the free-running indices pass `entries` often.
    for round in 0..7 {
        let fill = if round % 2 == 0 { 8 } else { 3 };
        for _ in 0..fill {
            assert_eq!(tx.push(&request(next)), Ok(Ok(())));
            next += 1;
        }
        if fill == 8 {
            assert_eq!(tx.push(&request(99)), Ok(Err(Full)), "full");
            assert_eq!(tx.space(), Ok(0));
        }
        while let Some(got) = rx.pop().unwrap() {
            assert_eq!(got.tag, want);
            want += 1;
        }
        assert_eq!(rx.ready(), Ok(0));
    }
    assert_eq!(want, next);
    // Responses go the other way on their own indices.
    let mut rtx = ring.response_producer();
    let mut rrx = ring.response_consumer();
    let done = Response {
        tag: 5,
        status: 0,
        len: 512,
    };
    assert_eq!(rtx.push(&done), Ok(Ok(())));
    assert_eq!(rrx.pop(), Ok(Some(done)));
}

#[test]
fn free_running_indices_wrap_at_u32_max() {
    let mut region = Region::new(size(8, 0));
    let ring = unsafe { Ring::init(region.ptr(), region.len(), BLOCK, 8) }.unwrap();
    let start = u32::MAX - 3;
    let header = unsafe { &*region.header() };
    header.req_head.store(start, Ordering::SeqCst);
    header.req_tail.store(start, Ordering::SeqCst);
    let mut tx = ring.request_producer();
    let mut rx = ring.request_consumer();
    for tag in 0..20 {
        assert_eq!(tx.push(&request(tag)), Ok(Ok(())));
        assert_eq!(rx.pop().unwrap().map(|r| r.tag), Some(tag));
    }
    assert_eq!(
        header.req_head.load(Ordering::SeqCst),
        start.wrapping_add(20)
    );
}

/// The region and the ring over it, shared by two threads.
struct Shared {
    _region: Region,
    ring: Ring<'static>,
}

#[test]
fn a_producer_and_a_consumer_racing_on_two_threads_keep_order() {
    const COUNT: u64 = 200_000;
    let mut region = Region::new(size(16, 0));
    let (ptr, len) = (region.ptr(), region.len());
    // SAFETY: the region lives in `Shared` as long as the ring.
    let ring = unsafe { Ring::init(ptr, len, BLOCK, 16) }.unwrap();
    let shared = Arc::new(Shared {
        _region: region,
        ring,
    });
    let producer = {
        let shared = shared.clone();
        thread::spawn(move || {
            let mut tx = shared.ring.request_producer();
            let mut tag = 0;
            while tag < COUNT {
                match tx.push(&request(tag)).unwrap() {
                    Ok(()) => tag += 1,
                    Err(Full) => thread::yield_now(),
                }
            }
        })
    };
    let mut rx = shared.ring.request_consumer();
    let mut want = 0;
    while want < COUNT {
        match rx.pop().unwrap() {
            Some(got) => {
                assert_eq!(got, request(want), "in order, whole");
                want += 1;
            }
            None => thread::yield_now(),
        }
    }
    producer.join().unwrap();
    assert_eq!(rx.pop(), Ok(None));
}

#[test]
fn a_hostile_producers_head_is_corrupt_never_a_read_out_of_bounds() {
    let mut region = Region::new(size(8, 0));
    let ring = unsafe { Ring::init(region.ptr(), region.len(), BLOCK, 8) }.unwrap();
    let header = unsafe { &*region.header() };
    let mut rx = ring.request_consumer();
    // A head more than `entries` ahead of the tail.
    for bad in [9, 1000, u32::MAX] {
        header.req_head.store(bad, Ordering::SeqCst);
        assert_eq!(rx.pop(), Err(Corrupt), "{bad}");
    }
    // A head that goes back.
    header.req_head.store(5, Ordering::SeqCst);
    assert_eq!(rx.ready(), Ok(5));
    header.req_head.store(3, Ordering::SeqCst);
    assert_eq!(rx.ready(), Err(Corrupt));
    // `entries` rewritten after attach is not believed: slots stay masked.
    unsafe { (*region.header()).entries = 4096 };
    header.req_head.store(5, Ordering::SeqCst);
    for _ in 0..5 {
        assert!(rx.pop().unwrap().is_some());
    }
}

#[test]
fn a_hostile_consumers_tail_is_corrupt_never_a_write_out_of_bounds() {
    let mut region = Region::new(size(8, 0));
    let ring = unsafe { Ring::init(region.ptr(), region.len(), BLOCK, 8) }.unwrap();
    let header = unsafe { &*region.header() };
    let mut tx = ring.request_producer();
    for tag in 0..4 {
        tx.push(&request(tag)).unwrap().unwrap();
    }
    // A tail past the head.
    header.req_tail.store(5, Ordering::SeqCst);
    assert_eq!(tx.push(&request(9)), Err(Corrupt));
    // A tail that goes back.
    header.req_tail.store(3, Ordering::SeqCst);
    assert_eq!(tx.space(), Ok(7));
    header.req_tail.store(1, Ordering::SeqCst);
    assert_eq!(tx.space(), Err(Corrupt));
    // Far out of range.
    header.req_tail.store(u32::MAX / 2, Ordering::SeqCst);
    assert_eq!(tx.push(&request(9)), Err(Corrupt));
}

#[test]
fn doorbell_wishes_are_each_sides_to_set() {
    let mut region = Region::new(size(8, 0));
    let ring = unsafe { Ring::init(region.ptr(), region.len(), VMEXIT, 8) }.unwrap();
    assert!(!ring.wants_requests() && !ring.wants_responses());
    ring.want_requests(true);
    ring.want_responses(true);
    assert!(ring.wants_requests() && ring.wants_responses());
    ring.want_requests(false);
    assert!(!ring.wants_requests());
}
