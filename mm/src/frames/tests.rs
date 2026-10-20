use super::*;
use alloc::vec;
use alloc::vec::Vec;

#[test]
fn starts_full_and_releases_ranges() {
    let mut storage = vec![0; FrameBitmap::words_for(200)];
    let mut bitmap = FrameBitmap::new(&mut storage, 200);
    assert_eq!(bitmap.free_count(), 0);
    assert_eq!(bitmap.allocate(), None);
    bitmap.release_range(10, 5);
    bitmap.release_range(12, 5); // overlap is counted once
    assert_eq!(bitmap.free_count(), 7);
    bitmap.release_range(195, 100); // clipped at the end
    assert_eq!(bitmap.free_count(), 12);
}

#[test]
fn allocates_every_free_frame_once() {
    let mut storage = vec![0; FrameBitmap::words_for(130)];
    let mut bitmap = FrameBitmap::new(&mut storage, 130);
    bitmap.release_range(0, 130);
    bitmap.reserve_range(64, 2);
    let mut got: Vec<u64> = core::iter::from_fn(|| bitmap.allocate()).collect();
    got.sort_unstable();
    assert_eq!(got.len(), 128);
    assert!(!got.contains(&64) && !got.contains(&65));
    assert!(got.iter().all(|&f| f < 130));
    assert_eq!(bitmap.free_count(), 0);
}

#[test]
fn free_detects_double_free_and_range_errors() {
    let mut storage = vec![0; 1];
    let mut bitmap = FrameBitmap::new(&mut storage, 64);
    bitmap.release_range(0, 64);
    let frame = bitmap.allocate().unwrap();
    assert_eq!(bitmap.free(frame), Ok(()));
    assert_eq!(bitmap.free(frame), Err(FrameError::DoubleFree));
    assert_eq!(bitmap.free(64), Err(FrameError::OutOfRange));
    assert_eq!(bitmap.free_count(), 64);
}

#[test]
fn contiguous_allocation_skips_holes_and_honours_alignment() {
    let mut storage = vec![0; FrameBitmap::words_for(256)];
    let mut bitmap = FrameBitmap::new(&mut storage, 256);
    bitmap.release_range(0, 256);
    bitmap.reserve_range(5, 1); // a hole at frame 5
    assert_eq!(bitmap.allocate_contiguous(8, 1), Some(6));
    assert_eq!(bitmap.allocate_contiguous(4, 1), Some(0));
    assert_eq!(bitmap.allocate_contiguous(16, 16), Some(16));
    assert_eq!(bitmap.free_count(), 256 - 1 - 8 - 4 - 16);
    assert!((16..32).all(|f| !bitmap.is_free(f)));
    assert_eq!(bitmap.allocate_contiguous(1000, 1), None);
    assert_eq!(bitmap.allocate_contiguous(0, 1), None);
}

#[test]
fn contiguous_free_restores_and_rejects_bad_ranges() {
    let mut storage = vec![0; FrameBitmap::words_for(64)];
    let mut bitmap = FrameBitmap::new(&mut storage, 64);
    bitmap.release_range(0, 64);
    let first = bitmap.allocate_contiguous(10, 2).unwrap();
    assert_eq!(bitmap.free_contiguous(first, 10), Ok(()));
    assert_eq!(bitmap.free_count(), 64);
    assert_eq!(
        bitmap.free_contiguous(first, 10),
        Err(FrameError::DoubleFree)
    );
    assert_eq!(bitmap.free_contiguous(60, 10), Err(FrameError::OutOfRange));
}

/// The search as it was, frame by frame, for comparison.
fn naive(bitmap: &FrameBitmap, count: u64, align: u64) -> Option<u64> {
    let mut start = 0;
    while start + count <= bitmap.total() {
        match (start..start + count).find(|&frame| !bitmap.is_free(frame)) {
            Some(used) => start = (used + 1).next_multiple_of(align),
            None => return Some(start),
        }
    }
    None
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut x = *state;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[test]
fn word_search_finds_what_the_frame_by_frame_search_found() {
    let mut state = 7;
    for round in 0..3_000 {
        let frames = 1 + splitmix(&mut state) % 700;
        let mut storage = vec![0; FrameBitmap::words_for(frames) + (round % 2)];
        let mut bitmap = FrameBitmap::new(&mut storage, frames);
        bitmap.release_range(0, frames);
        // Used frames at a random density, sometimes in runs.
        let density = splitmix(&mut state) % 100;
        let mut frame = 0;
        while frame < frames {
            let run = 1 + splitmix(&mut state) % 9;
            if splitmix(&mut state) % 100 < density {
                bitmap.reserve_range(frame, run);
            }
            frame += run;
        }
        for _ in 0..6 {
            let count = 1 + splitmix(&mut state) % 130;
            let align = [1, 2, 3, 8, 64, 100, 128, 512][(splitmix(&mut state) % 8) as usize];
            let free = bitmap.free_count();
            let expected = (count <= free)
                .then(|| naive(&bitmap, count, align))
                .flatten();
            assert_eq!(
                bitmap.allocate_contiguous(count, align),
                expected,
                "round {round}: {count} frames aligned {align} in {frames}"
            );
            if let Some(first) = expected {
                assert!((first..first + count).all(|f| !bitmap.is_free(f)));
                assert_eq!(bitmap.free_count(), free - count);
                if splitmix(&mut state).is_multiple_of(3) {
                    assert_eq!(bitmap.free_contiguous(first, count), Ok(()));
                    assert_eq!(bitmap.free_count(), free);
                }
            }
        }
    }
}

#[test]
fn finds_every_free_2_mib_run_in_8_gib() {
    const FRAMES: u64 = 8 << 18;
    const RUN: u64 = 512;
    let mut storage = vec![0; FrameBitmap::words_for(FRAMES)];
    let mut bitmap = FrameBitmap::new(&mut storage, FRAMES);
    bitmap.release_range(0, FRAMES);
    // A used frame every 6 MiB lands in every third aligned 2 MiB window.
    for hole in (7..FRAMES).step_by(3 * RUN as usize) {
        bitmap.reserve_range(hole, 1);
    }
    let windows = FRAMES / RUN;
    let spoiled = windows.div_ceil(3);
    let mut previous = None;
    let mut found = 0;
    while let Some(first) = bitmap.allocate_contiguous(RUN, RUN) {
        assert_eq!(first % RUN, 0);
        assert_ne!((first / RUN) % 3, 0, "window {} holds a hole", first / RUN);
        assert!(previous < Some(first), "first fit goes up");
        previous = Some(first);
        found += 1;
    }
    assert_eq!(found, windows - spoiled);
    assert_eq!(bitmap.free_count(), FRAMES - spoiled - found * RUN);
    // Freeing one run makes exactly it the next one found.
    let again = RUN * 4;
    assert_eq!(bitmap.free_contiguous(again, RUN), Ok(()));
    assert_eq!(bitmap.allocate_contiguous(RUN, RUN), Some(again));
}

#[test]
fn a_free_below_the_scan_start_is_found_again() {
    let mut storage = vec![0; FrameBitmap::words_for(1024)];
    let mut bitmap = FrameBitmap::new(&mut storage, 1024);
    bitmap.release_range(0, 1024);
    assert_eq!(bitmap.allocate_contiguous(700, 1), Some(0));
    assert_eq!(bitmap.allocate_contiguous(10, 1), Some(700));
    bitmap.free(3).unwrap();
    assert_eq!(bitmap.allocate_contiguous(1, 1), Some(3));
    assert_eq!(bitmap.free_contiguous(100, 64), Ok(()));
    // 128..192 is only half free; the first whole aligned window is past 710.
    assert_eq!(bitmap.allocate_contiguous(64, 64), Some(768));
    assert_eq!(bitmap.allocate_contiguous(60, 4), Some(100));
    assert_eq!(bitmap.free_contiguous(0, 0), Ok(()));
    assert_eq!(
        bitmap.free_contiguous(1000, 25),
        Err(FrameError::OutOfRange)
    );
    assert_eq!(bitmap.free_contiguous(709, 2), Err(FrameError::DoubleFree));
}

#[test]
fn a_run_can_be_asked_for_above_a_frame() {
    let mut storage = vec![0; FrameBitmap::words_for(4096)];
    let mut bitmap = FrameBitmap::new(&mut storage, 4096);
    bitmap.release_range(0, 4096);
    assert_eq!(bitmap.allocate_contiguous_from(1000, 512, 512), Some(1024));
    assert_eq!(bitmap.allocate_contiguous_from(1000, 512, 512), Some(1536));
    // The ordinary search still starts from the bottom.
    assert_eq!(bitmap.allocate_contiguous(512, 512), Some(0));
    assert_eq!(bitmap.allocate_contiguous_from(3585, 512, 512), None);
    assert_eq!(bitmap.free_count(), 4096 - 3 * 512);
}

#[test]
fn reuses_freed_frames_without_leaking() {
    let mut storage = vec![0; FrameBitmap::words_for(256)];
    let mut bitmap = FrameBitmap::new(&mut storage, 256);
    bitmap.release_range(1, 255);
    let before = bitmap.free_count();
    for _ in 0..10_000 {
        let a = bitmap.allocate().unwrap();
        let b = bitmap.allocate().unwrap();
        bitmap.free(a).unwrap();
        bitmap.free(b).unwrap();
    }
    assert_eq!(bitmap.free_count(), before);
    assert!(!bitmap.is_free(0));
}
