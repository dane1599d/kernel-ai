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
        self.search(self.low as u64 * 64, count, align, true, self.frames)
    }

    /// As [`Self::allocate_contiguous`], but the whole run ends at or below frame
    /// `limit`: DMA for a device that cannot address memory above it (4 GiB is
    /// frame 1 << 20, ADR 0007's `Zone::Below4G`).
    pub fn allocate_contiguous_below(&mut self, limit: u64, count: u64, align: u64) -> Option<u64> {
        self.search(self.low as u64 * 64, count, align, true, limit)
    }

    /// As [`Self::allocate_contiguous`], but the run starts at frame `lowest` or
    /// above: for a test that needs memory above 4 GiB.
    pub fn allocate_contiguous_from(&mut self, lowest: u64, count: u64, align: u64) -> Option<u64> {
        let from = lowest.max(self.low as u64 * 64);
        self.search(
            from,
            count,
            align,
            from == self.low as u64 * 64,
            self.frames,
        )
    }

    /// The search behind all three; `from_low` says the scan starts at the lower
    /// bound, so the first free frame it meets may raise the bound. The run must
    /// end at or below `limit`.
    fn search(
        &mut self,
        mut from: u64,
        count: u64,
        align: u64,
        from_low: bool,
        limit: u64,
    ) -> Option<u64> {
        if count == 0 || align == 0 || count > self.free {
            return None;
        }
        let mut first_free_seen = !from_low;
        loop {
            let free = self.next_free(from)?;
            if !first_free_seen {
                // Every word below this one is full.
                self.low = (free / 64) as usize;
                first_free_seen = true;
            }
            let start = free.checked_next_multiple_of(align)?;
            let end = start
                .checked_add(count)
                .filter(|&end| end <= self.frames.min(limit))?;
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
mod tests;
