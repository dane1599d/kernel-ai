//! Whether a window's picture scrolled, for `desk-wheel-app`: a redraw proves
//! the app took the wheel, not that its content moved. Before the wheel
//! `desktop` keeps a hash of each row of a band of the window's pixels
//! ([`row_hashes`]); at the window's next picture [`shift`] looks for a
//! vertical move of the content that explains the rows that changed.

use alloc::vec::Vec;

/// How many changed rows one shift must explain to count as a scroll: more than
/// a caret or a hover highlight redraws, fewer than a line of text.
pub const MIN_ROWS: usize = 12;

/// A hash of `width` px of each row of `px` from column `x0`, `stride` px a
/// row: FNV-1a over the words. A row past `px` hashes what is there.
pub fn row_hashes(px: &[u32], stride: usize, x0: usize, width: usize) -> Vec<u64> {
    if stride == 0 {
        return Vec::new();
    }
    px.chunks(stride)
        .map(|row| {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for &word in row.iter().skip(x0).take(width) {
                hash = (hash ^ u64::from(word)).wrapping_mul(0x100_0000_01b3);
            }
            hash
        })
        .collect()
}

/// The vertical move, in rows, that best explains how `after` differs from
/// `before`: `d` such that many rows `i` that changed (`after[i] != before[i]`)
/// equal row `i + d` of `before` (content moved up by `d` when `d` is
/// positive, down when negative). `Some((d, rows))` when `rows` reaches
/// [`MIN_ROWS`], `None` when the picture changed in place or not at all. A
/// row that did not change, blank ones included, explains nothing.
pub fn shift(before: &[u64], after: &[u64]) -> Option<(i32, usize)> {
    let rows = before.len().min(after.len());
    let mut best: Option<(i32, usize)> = None;
    for d in -((rows / 2) as i32)..=(rows / 2) as i32 {
        if d == 0 {
            continue;
        }
        let explained = (0..rows)
            .filter(|&i| {
                let from = i as i32 + d;
                after[i] != before[i]
                    && from >= 0
                    && (from as usize) < rows
                    && after[i] == before[from as usize]
            })
            .count();
        if explained >= MIN_ROWS && best.is_none_or(|(_, most)| explained > most) {
            best = Some((d, explained));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Rows of distinct content: `n` rows, row `i` hashing to `seed + i`.
    fn page(seed: u64, n: usize) -> Vec<u64> {
        (0..n as u64).map(|i| seed + i).collect()
    }

    #[test]
    fn rows_are_hashed_over_the_band_only() {
        let px = [1, 2, 3, 4, 5, 6, 7, 8];
        let a = row_hashes(&px, 4, 1, 2);
        // Columns 1 and 2 of each row: 2,3 and 6,7.
        assert_eq!(a.len(), 2);
        assert_ne!(a[0], a[1]);
        let same_band = [9, 2, 3, 9, 9, 6, 7, 9];
        assert_eq!(row_hashes(&same_band, 4, 1, 2), a);
        assert!(row_hashes(&px, 0, 0, 2).is_empty());
    }

    #[test]
    fn content_moved_up_is_a_positive_shift() {
        let before = page(1000, 100);
        // Three rows up: after[i] is before[i + 3], the new rows at the foot.
        let mut after: Vec<u64> = before[3..].to_vec();
        after.extend([5000, 5001, 5002]);
        assert_eq!(shift(&before, &after), Some((3, 97)));
    }

    #[test]
    fn content_moved_down_is_a_negative_shift() {
        let before = page(1000, 100);
        let mut after = vec![7000, 7001];
        after.extend_from_slice(&before[..98]);
        assert_eq!(shift(&before, &after), Some((-2, 98)));
    }

    #[test]
    fn a_redraw_in_place_is_no_shift() {
        let before = page(1000, 100);
        assert_eq!(shift(&before, &before), None);
        // A hover highlight or a caret: a few rows change where they are.
        let mut after = before.clone();
        for row in &mut after[40..46] {
            *row += 90_000;
        }
        assert_eq!(shift(&before, &after), None);
        // A new picture that shares nothing with the old is no shift either.
        assert_eq!(shift(&before, &page(9000, 100)), None);
    }

    #[test]
    fn blank_rows_do_not_pass_for_a_scroll() {
        // A list that is blank but for one line: the line moves 3 rows, which
        // explains two changed rows, not twelve.
        let mut before = vec![42; 100];
        before[10] = 1;
        let mut after = vec![42; 100];
        after[7] = 1;
        assert_eq!(shift(&before, &after), None);
    }
}
