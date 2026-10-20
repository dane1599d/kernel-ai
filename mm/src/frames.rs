//! Physical frame allocator: one bit per 4 KiB frame, set means in use.

/// Errors from freeing or reserving frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    OutOfRange,
    DoubleFree,
}

/// Bitmap over frame numbers `0..frames`. Every frame starts in use; usable
/// memory is released with [`FrameBitmap::release_range`].
pub struct FrameBitmap<'a> {
    words: &'a mut [u64],
    frames: u64,
    free: u64,
    /// Word index where the next search starts (next-fit).
    hint: usize,
    /// No word below this one has a free frame: every free lowers it, and a
    /// contiguous search that finds the first free frame raises it.
    low: usize,
}

impl<'a> FrameBitmap<'a> {
    /// Words needed to track `frames` frames.
    pub const fn words_for(frames: u64) -> usize {
        frames.div_ceil(64) as usize
    }

    /// Takes over `words` (at least [`Self::words_for`] long) and marks every frame used.
    pub fn new(words: &'a mut [u64], frames: u64) -> Self {
        assert!(
            words.len() >= Self::words_for(frames),
            "bitmap storage too small"
        );
        words.fill(u64::MAX);
        Self {
            words,
            frames,
            free: 0,
            hint: 0,
            low: 0,
        }
    }

    pub fn total(&self) -> u64 {
        self.frames
    }

    pub fn free_count(&self) -> u64 {
        self.free
    }

    pub fn is_free(&self, frame: u64) -> bool {
        frame < self.frames && self.words[(frame / 64) as usize] & (1 << (frame % 64)) == 0
    }

    /// Marks `count` frames from `first` free. Frames already free are left alone,
    /// and frames past the end are ignored, so overlapping usable regions are harmless.
    pub fn release_range(&mut self, first: u64, count: u64) {
        let end = first.saturating_add(count).min(self.frames);
        for frame in first..end {
            if !self.is_free(frame) {
                self.clear(frame);
                self.free += 1;
            }
        }
    }

    /// Marks `count` frames from `first` used, e.g. for the bitmap's own storage.
    pub fn reserve_range(&mut self, first: u64, count: u64) {
        let end = first.saturating_add(count).min(self.frames);
        for frame in first..end {
            if self.is_free(frame) {
                self.set(frame);
                self.free -= 1;
            }
        }
    }

    /// Returns a free frame number and marks it used.
    pub fn allocate(&mut self) -> Option<u64> {
        let words = self.words.len();
        for step in 0..words {
            let index = (self.hint + step) % words;
            let word = self.words[index];
            if word == u64::MAX {
                continue;
            }
            let frame = index as u64 * 64 + u64::from((!word).trailing_zeros());
            if frame >= self.frames {
                continue;
            }
            self.set(frame);
            self.free -= 1;
            self.hint = index;
            return Some(frame);
        }
        None
    }

    /// Finds `count` consecutive free frames whose first frame number is a multiple of
    /// `align`, marks them used, and returns the first. First fit from frame 0, so
    /// large early allocations (the heap, DMA rings) come from low memory.
    ///
    /// The search works a word (64 frames) at a time: it skips used words to the
    /// next free frame, rounds up to the alignment, and checks the run from its end
    /// backwards; a used frame in the run means no start up to that frame can work,
    /// so the search resumes just past it. Thousands of 2 MiB runs (512 frames
    /// aligned 512) in an 8 GiB bitmap take milliseconds, not minutes.
    pub fn allocate_contiguous(&mut self, count: u64, align: u64) -> Option<u64> {
        if count == 0 || align == 0 || count > self.free {
            return None;
        }
        let mut from = self.low as u64 * 64;
        let mut first_free_seen = false;
        loop {
            let free = self.next_free(from)?;
            if !first_free_seen {
                // Every word below this one is full.
                self.low = (free / 64) as usize;
                first_free_seen = true;
            }
            let start = free.checked_next_multiple_of(align)?;
            let end = start.checked_add(count).filter(|&end| end <= self.frames)?;
            match self.last_used(start, end) {
                Some(used) => from = used + 1,
                None => {
                    self.fill(start, end, true);
                    self.free -= count;
                    return Some(start);
                }
            }
        }
    }

    /// Frees `count` frames starting at `first`, as allocated by `allocate_contiguous`.
    pub fn free_contiguous(&mut self, first: u64, count: u64) -> Result<(), FrameError> {
        let end = first
            .checked_add(count)
            .filter(|&end| end <= self.frames)
            .ok_or(FrameError::OutOfRange)?;
        if count != 0 && self.first_free(first, end).is_some() {
            return Err(FrameError::DoubleFree);
        }
        self.fill(first, end, false);
        self.free += count;
        Ok(())
    }

    /// The first free frame at or after `from`, a word at a time.
    fn next_free(&self, from: u64) -> Option<u64> {
        if from >= self.frames {
            return None;
        }
        let mut index = (from / 64) as usize;
        // Frames below `from` in its word count as used.
        let mut word = self.words[index] | low_bits(from % 64);
        loop {
            if word != u64::MAX {
                let frame = index as u64 * 64 + u64::from((!word).trailing_zeros());
                return (frame < self.frames).then_some(frame);
            }
            index += 1;
            if index as u64 * 64 >= self.frames {
                return None;
            }
            word = self.words[index];
        }
    }

    /// The highest used frame in `start..end` (`start < end <= frames`), checking
    /// whole words from the end.
    fn last_used(&self, start: u64, end: u64) -> Option<u64> {
        let mut index = ((end - 1) / 64) as usize;
        loop {
            let word = self.words[index] & range_bits(index, start, end);
            if word != 0 {
                return Some(index as u64 * 64 + 63 - u64::from(word.leading_zeros()));
            }
            if index as u64 * 64 <= start {
                return None;
            }
            index -= 1;
        }
    }

    /// The lowest free frame in `start..end` (`start < end <= frames`).
    fn first_free(&self, start: u64, end: u64) -> Option<u64> {
        let last = ((end - 1) / 64) as usize;
        (start as usize / 64..=last).find_map(|index| {
            let free = !self.words[index] & range_bits(index, start, end);
            (free != 0).then(|| index as u64 * 64 + u64::from(free.trailing_zeros()))
        })
    }

    /// Marks `start..end` used (`true`) or free, a word at a time.
    fn fill(&mut self, start: u64, end: u64, used: bool) {
        if start >= end {
            return;
        }
        for index in start as usize / 64..=((end - 1) / 64) as usize {
            let bits = range_bits(index, start, end);
            if used {
                self.words[index] |= bits;
            } else {
                self.words[index] &= !bits;
                self.low = self.low.min(index);
            }
        }
    }

    pub fn free(&mut self, frame: u64) -> Result<(), FrameError> {
        if frame >= self.frames {
            return Err(FrameError::OutOfRange);
        }
        if self.is_free(frame) {
            return Err(FrameError::DoubleFree);
        }
        self.clear(frame);
        self.free += 1;
        Ok(())
    }

    fn set(&mut self, frame: u64) {
        self.words[(frame / 64) as usize] |= 1 << (frame % 64);
    }

    fn clear(&mut self, frame: u64) {
        let index = (frame / 64) as usize;
        self.words[index] &= !(1 << (frame % 64));
        self.low = self.low.min(index);
    }
}

/// The `n` lowest bits set (`n` < 64).
const fn low_bits(n: u64) -> u64 {
    (1 << n) - 1
}

/// The bits of word `index` that fall in frames `start..end`.
fn range_bits(index: usize, start: u64, end: u64) -> u64 {
    let base = index as u64 * 64;
    let lo = start.saturating_sub(base).min(64);
    let hi = end.saturating_sub(base).min(64);
    let upto = |n: u64| if n == 64 { u64::MAX } else { low_bits(n) };
    upto(hi) & !upto(lo)
}

#[cfg(test)]
mod tests {
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
}
