//! Screen rectangles in pixels.

/// A rectangle of `width` by `height` pixels whose top-left pixel is `x`, `y`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The whole of a `width` by `height` screen.
    pub const fn screen(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The smallest rectangle holding both; an empty one adds nothing.
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        Rect::new(x, y, right - x, bottom - y)
    }

    /// The part inside a `width` by `height` screen, empty if none is.
    pub fn clamp(self, width: u32, height: u32) -> Rect {
        if self.x >= width || self.y >= height {
            return Rect::default();
        }
        Rect::new(
            self.x,
            self.y,
            self.width.min(width - self.x),
            self.height.min(height - self.y),
        )
    }
}

/// `rect` in bands of at most `rows` rows, top to bottom, each passed to `band`
/// until it returns false. Returns whether every band was passed and taken.
///
/// A copy too big to make under one lock (a full screen, packed pixel by pixel)
/// takes the lock per band: whoever waits for that lock waits one band, and
/// `band` checks again, under it, whether to go on.
pub fn each_band(rect: Rect, rows: u32, mut band: impl FnMut(Rect) -> bool) -> bool {
    let rows = rows.max(1);
    let mut y = rect.y;
    let end = rect.y.saturating_add(rect.height);
    while y < end {
        let height = rows.min(end - y);
        if !band(Rect::new(rect.x, y, rect.width, height)) {
            return false;
        }
        y += height;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::Rect;
    use super::each_band;
    use std::vec::Vec;

    #[test]
    fn union_covers_both_and_ignores_empty() {
        let a = Rect::new(10, 20, 5, 5);
        let b = Rect::new(0, 30, 100, 2);
        assert_eq!(a.union(b), Rect::new(0, 20, 100, 12));
        assert_eq!(a.union(Rect::default()), a);
        assert_eq!(Rect::default().union(b), b);
    }

    #[test]
    fn clamp_keeps_the_part_on_screen() {
        assert_eq!(
            Rect::new(1270, 790, 64, 64).clamp(1280, 800),
            Rect::new(1270, 790, 10, 10)
        );
        assert!(Rect::new(1280, 0, 8, 8).clamp(1280, 800).is_empty());
        assert_eq!(
            Rect::screen(1280, 800).clamp(1280, 800),
            Rect::screen(1280, 800)
        );
    }

    #[test]
    fn bands_cover_the_rectangle_once_top_to_bottom() {
        let mut seen = Vec::new();
        assert!(each_band(Rect::new(3, 10, 7, 130), 64, |b| {
            seen.push(b);
            true
        }));
        assert_eq!(
            seen,
            [
                Rect::new(3, 10, 7, 64),
                Rect::new(3, 74, 7, 64),
                Rect::new(3, 138, 7, 2)
            ]
        );
        let mut none = 0;
        assert!(each_band(Rect::new(0, 5, 9, 0), 64, |_| {
            none += 1;
            true
        }));
        assert_eq!(none, 0, "an empty rectangle has no band");
    }

    #[test]
    fn a_band_that_says_stop_ends_the_copy_before_the_next() {
        // As when the screen is taken from its holder mid-flush: the band
        // whose check fails, and every band after it, is not copied.
        let (mut checked, mut copied) = (0, 0);
        let showing = |n: u32| n < 2;
        let all = each_band(Rect::new(0, 0, 1920, 1080), 64, |_| {
            checked += 1;
            if !showing(checked - 1) {
                return false;
            }
            copied += 1;
            true
        });
        assert!(!all);
        assert_eq!((checked, copied), (3, 2));
    }

    #[test]
    fn zero_rows_a_band_is_taken_as_one() {
        let mut n = 0;
        each_band(Rect::new(0, 0, 4, 3), 0, |_| {
            n += 1;
            true
        });
        assert_eq!(n, 3);
    }
}
