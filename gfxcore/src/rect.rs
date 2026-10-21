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

#[cfg(test)]
mod tests {
    use super::Rect;

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
}
