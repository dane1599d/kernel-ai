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
        self.words[(frame / 64) as usize] &= !(1 << (frame % 64));
    }
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
