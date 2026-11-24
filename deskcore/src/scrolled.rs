//! Whether a window's picture scrolled, for `desk-wheel-app`: a redraw proves
//! the app took the wheel, not that its content moved. Before the wheel
//! `desktop` keeps a hash of each row of a band of the window's pixels
//! ([`row_hashes`]); at the window's next picture [`shift`] looks for a
//! vertical move of the content that explains the rows that changed.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use surfabi::proto::SCALE_ONE;

/// How many changed rows one shift must explain to count as a scroll: more than
/// a caret or a hover highlight redraws, fewer than a line of text.
pub const MIN_ROWS: usize = 12;

/// `logical` px of a window's content in the buffer's px at `scale`, which is
/// in 120ths as a window's scale is (0 until the client says: 1x).
pub fn buffer_px(logical: usize, scale: u32) -> usize {
    let scale = match scale {
        0 => SCALE_ONE,
        s => s,
    };
    logical * scale as usize / SCALE_ONE as usize
}

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
/// row that did not change explains nothing, and nor does a row of `before`
/// that is not the only one of its hash (background, a selection's fill): a
/// row that was cleared or filled in place would seem to have come from it.
pub fn shift(before: &[u64], after: &[u64]) -> Option<(i32, usize)> {
    let rows = before.len().min(after.len());
    let mut seen: BTreeMap<u64, usize> = BTreeMap::new();
    for &hash in before {
        *seen.entry(hash).or_default() += 1;
    }
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
                    && seen[&before[from as usize]] == 1
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
    fn a_band_is_in_buffer_px_at_the_windows_scale() {
        assert_eq!(buffer_px(8, 120), 8);
        assert_eq!(buffer_px(88, 120), 88);
        assert_eq!(buffer_px(8, 240), 16);
        assert_eq!(buffer_px(88, 240), 176);
        assert_eq!(buffer_px(8, 180), 12);
        // No scale said yet is 1x. The scale is in 120ths, not a whole number.
        assert_eq!(buffer_px(88, 0), 88);
    }

    #[test]
    fn the_band_a_scaled_window_hashes_is_the_same_content() {
        // The same picture at 1x and at 2x (each px doubled across): the band's
        // rows hash the same only if the band is scaled with it.
        let wide = |scale: u32| -> Vec<u64> {
            let factor = scale as usize / 120;
            let stride = 200 * factor;
            let mut px = Vec::new();
            for row in 0..10u32 {
                for x in 0..200u32 {
                    let value = if (8..96).contains(&x) {
                        row + 1
                    } else {
                        0xFFFF
                    };
                    px.extend(core::iter::repeat_n(value, factor));
                }
            }
            row_hashes(&px, stride, buffer_px(8, scale), buffer_px(88, scale))
        };
        let one = wide(120);
        assert_eq!(one.len(), 10);
        assert!(one.windows(2).all(|w| w[0] != w[1]), "a row per hash at 1x");
        assert_eq!(one.len(), wide(240).len());
        assert!(wide(240).windows(2).all(|w| w[0] != w[1]), "and at 2x");
    }

    #[test]
    fn a_list_blanked_in_place_is_no_scroll() {
        // Forty distinct rows, then blank: the list is cleared where it is.
        let mut before = page(1000, 40);
        before.extend(vec![42; 60]);
        let after = vec![42; 100];
        assert_eq!(shift(&before, &after), None);
    }

    #[test]
    fn a_selection_that_moves_is_no_scroll() {
        // The selection's fill, 30 rows of one hash, moves down by 30.
        let mut before = page(1000, 100);
        let mut after = before.clone();
        for row in &mut before[30..60] {
            *row = 7;
        }
        for row in &mut after[60..90] {
            *row = 7;
        }
        assert_eq!(shift(&before, &after), None);
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
