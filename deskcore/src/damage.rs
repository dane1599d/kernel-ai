//! What must be drawn again: at most [`MAX`] rectangles a frame, their
//! bounding box past that (ADR 0005 §5). Every rectangle is cut to the screen;
//! one inside another is dropped, and two that overlap become their union when
//! that covers little more than the two did.

use crate::geom::Rect;
use alloc::vec::Vec;

/// Rectangles one frame redraws, before they merge into their bounding box.
pub const MAX: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Damage {
    rects: Vec<Rect>,
    screen: Rect,
}

impl Damage {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            rects: Vec::new(),
            screen: Rect::new(0, 0, width, height),
        }
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        self.screen = Rect::new(0, 0, width, height);
        self.rects.clear();
        self.rects.push(self.screen);
    }

    pub fn add(&mut self, rect: Rect) {
        let Some(mut rect) = rect.intersect(&self.screen) else {
            return;
        };
        if self.rects.iter().any(|r| r.intersect(&rect) == Some(rect)) {
            return;
        }
        // Fold in what overlaps it, while the union stays tight.
        let mut n = 0;
        while n < self.rects.len() {
            let other = self.rects[n];
            let union = other.union(&rect);
            let tight = union.area() <= (other.area() + rect.area()) * 5 / 4;
            if other.intersect(&rect).is_some() && tight {
                rect = union;
                self.rects.swap_remove(n);
                n = 0;
                continue;
            }
            n += 1;
        }
        self.rects.push(rect);
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// The frame's damage, and a clean slate.
    pub fn take(&mut self) -> Vec<Rect> {
        let rects = core::mem::take(&mut self.rects);
        if rects.len() <= MAX {
            return rects;
        }
        let whole = rects.iter().fold(Rect::default(), |acc, r| acc.union(r));
        alloc::vec![whole]
    }
}
