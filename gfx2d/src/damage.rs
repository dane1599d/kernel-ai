//! Damage: what changed since the last frame, as at most `N` rectangles, and past
//! that their bounding box. `surfabi`'s header page carries 32 for a client;
//! `desktop` composes at most 16 (ADR 0005 §5).

use crate::canvas::Rect;

#[derive(Clone, Debug)]
pub struct Damage<const N: usize> {
    rects: [Rect; N],
    len: usize,
}

impl<const N: usize> Default for Damage<N> {
    fn default() -> Self {
        Damage {
            rects: [Rect::default(); N],
            len: 0,
        }
    }
}

impl<const N: usize> Damage<N> {
    /// Adds `r`: merged into any rectangle it overlaps, else kept apart; past
    /// `N`, everything collapses to one bounding box.
    pub fn add(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        let mut r = r;
        // Merging can make the union overlap others, so repeat until it doesn't.
        let mut i = 0;
        while i < self.len {
            if !self.rects[i].intersect(&r).is_empty() {
                r = r.union(&self.rects[i]);
                self.len -= 1;
                self.rects[i] = self.rects[self.len];
                i = 0;
            } else {
                i += 1;
            }
        }
        if self.len < N {
            self.rects[self.len] = r;
            self.len += 1;
        } else {
            let all = self.rects[..self.len].iter().fold(r, |a, b| a.union(b));
            self.rects[0] = all;
            self.len = 1;
        }
    }

    pub fn rects(&self) -> &[Rect] {
        &self.rects[..self.len]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// The rectangles as `surfabi`'s header page stores them, `u16` x, y, w, h,
    /// clipped to a `width` x `height` buffer.
    pub fn as_u16(&self, width: u16, height: u16) -> impl Iterator<Item = [u16; 4]> + '_ {
        let screen = Rect::new(0, 0, i32::from(width), i32::from(height));
        self.rects().iter().filter_map(move |r| {
            let r = r.intersect(&screen);
            (!r.is_empty()).then_some([r.x as u16, r.y as u16, r.w as u16, r.h as u16])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_damage_merges_and_apart_stays_apart() {
        let mut d = Damage::<4>::default();
        d.add(Rect::new(0, 0, 10, 10));
        d.add(Rect::new(5, 5, 10, 10));
        d.add(Rect::new(100, 100, 5, 5));
        assert_eq!(d.rects().len(), 2);
        assert!(d.rects().contains(&Rect::new(0, 0, 15, 15)));
        // A rect bridging two merges all three.
        d.add(Rect::new(10, 10, 95, 95));
        assert_eq!(d.rects(), &[Rect::new(0, 0, 105, 105)]);
    }

    #[test]
    fn past_n_rectangles_it_is_one_bounding_box() {
        let mut d = Damage::<3>::default();
        for i in 0..4 {
            d.add(Rect::new(i * 20, 0, 5, 5));
        }
        assert_eq!(d.rects(), &[Rect::new(0, 0, 65, 5)]);
    }

    #[test]
    fn the_header_form_is_clipped() {
        let mut d = Damage::<32>::default();
        d.add(Rect::new(-5, 10, 20, 20));
        d.add(Rect::new(2000, 0, 5, 5));
        assert_eq!(
            d.as_u16(1920, 1080).collect::<std::vec::Vec<_>>(),
            [[0, 10, 15, 20]]
        );
    }
}
