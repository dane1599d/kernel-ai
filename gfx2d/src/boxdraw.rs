//! Box drawing and block elements, U+2500-259F, drawn by code per cell (ADR 0005
//! §5): lines meet exactly at cell edges at every size and scale, which glyphs
//! from a font do not. Written here from the Unicode chart, not from a font.
//!
//! - The line characters (U+2500-254B, U+2550-256C, U+2574-257F) are four arms
//!   from the cell's centre, each none, light, heavy or double ([`ARMS`]);
//!   U+2504-250B and U+254C-254F are the same arms dashed.
//! - U+256D-2570 are rounded corners, U+2571-2573 diagonals.
//! - U+2580-259F are blocks: eighths, halves, quadrants and three shades.

use crate::canvas::{Canvas, Rect, tint};

/// An arm's weight.
const NONE: u8 = 0;
const LIGHT: u8 = 1;
const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

/// U+2500-257F's arms as up, right, down, left digits (0 none, 1 light, 2 heavy,
/// 3 double); `-` for the arcs and diagonals, drawn apart.
const ARMS: [&str; 128] = [
    "0101", "0202", "1010", "2020", "0101", "0202", "1010", "2020", // 2500
    "0101", "0202", "1010", "2020", "0110", "0210", "0120", "0220", // 2508
    "0011", "0012", "0021", "0022", "1100", "1200", "2100", "2200", // 2510
    "1001", "1002", "2001", "2002", "1110", "1210", "2110", "1120", // 2518
    "2120", "2210", "1220", "2220", "1011", "1012", "2011", "1021", // 2520
    "2021", "2012", "1022", "2022", "0111", "0112", "0211", "0212", // 2528
    "0121", "0122", "0221", "0222", "1101", "1102", "1201", "1202", // 2530
    "2101", "2102", "2201", "2202", "1111", "1112", "1211", "1212", // 2538
    "2111", "1121", "2121", "2112", "2211", "1122", "1221", "2212", // 2540
    "1222", "2122", "2221", "2222", "0101", "0202", "1010", "2020", // 2548
    "0303", "3030", "0310", "0130", "0330", "0013", "0031", "0033", // 2550
    "1300", "3100", "3300", "1003", "3001", "3003", "1310", "3130", // 2558
    "3330", "1013", "3031", "3033", "0313", "0131", "0333", "1303", // 2560
    "3101", "3303", "1313", "3131", "3333", "-", "-", "-", // 2568
    "-", "-", "-", "-", "0001", "1000", "0100", "0010", // 2570
    "0002", "2000", "0200", "0020", "0201", "1020", "0102", "2010", // 2578
];

/// How many dashes an arm is cut into, for the dashed lines.
fn dashes(c: u32) -> u32 {
    match c {
        0x2504..=0x2507 => 3,
        0x2508..=0x250B => 4,
        0x254C..=0x254F => 2,
        _ => 0,
    }
}

/// Whether `c` is drawn here rather than from a font.
pub fn handles(c: char) -> bool {
    (0x2500..=0x259F).contains(&(c as u32))
}

/// The cell's pixels at coverage `a(x, y)` (0-255) in `color`, inside `clip`.
fn paint(c: &mut Canvas<'_>, cell: Rect, clip: Rect, color: u32, a: impl Fn(i32, i32) -> u32) {
    let r = cell.intersect(&clip).intersect(&c.bounds());
    for y in r.y..r.bottom() {
        let row = y as usize * c.stride_px as usize;
        for x in r.x..r.right() {
            let cov = a(x - cell.x, y - cell.y).min(255);
            if cov != 0 {
                let p = &mut c.pixels[row + x as usize];
                *p = tint(color, cov, *p);
            }
        }
    }
}

fn fill(c: &mut Canvas<'_>, cell: Rect, clip: Rect, color: u32, r: Rect) {
    let r = r.offset(cell.x, cell.y).intersect(&cell);
    paint(c, r, clip, color, |_, _| 255);
}

/// Draws `ch` filling `cell` in `color`, clipped to `clip`; false when `ch` is
/// not U+2500-259F.
pub fn draw(c: &mut Canvas<'_>, clip: Rect, cell: Rect, ch: char, color: u32) -> bool {
    let code = ch as u32;
    if !handles(ch) || cell.is_empty() {
        return false;
    }
    let w = cell.w;
    let light = (w / 8).max(1);
    let heavy = (light * 2).max(2);
    if code >= 0x2580 {
        blocks(c, clip, cell, code, color);
        return true;
    }
    match code {
        0x256D..=0x2570 => arc(c, clip, cell, code, color, light),
        0x2571..=0x2573 => diagonal(c, clip, cell, code, color, light),
        _ => {
            let arms: [u8; 4] =
                core::array::from_fn(|i| ARMS[(code - 0x2500) as usize].as_bytes()[i] - b'0');
            lines(c, clip, cell, arms, dashes(code), color, (light, heavy));
        }
    }
    true
}

/// The four arms. Each runs from the centre to its edge, the width of its own
/// weight, and overlaps the centre by half the widest crossing arm, so corners
/// and crossings join without gaps. A double arm is two light lines `light`
/// apart either side of the centre line.
fn lines(
    c: &mut Canvas<'_>,
    clip: Rect,
    cell: Rect,
    arms: [u8; 4],
    dash: u32,
    color: u32,
    (light, heavy): (i32, i32),
) {
    let (w, h) = (cell.w, cell.h);
    let (cx, cy) = (w / 2, h / 2);
    let width = |weight: u8| match weight {
        LIGHT => light,
        HEAVY => heavy,
        DOUBLE => 3 * light,
        _ => 0,
    };
    let [up, right, down, left] = arms;
    if dash > 0 {
        // A dashed character is one straight line, cut into `dash` dashes along
        // the whole cell, each with a gap of a third of its share.
        let horizontal_line = left != NONE;
        let t = width(left.max(up));
        let len = if horizontal_line { w } else { h };
        let n = dash as i32;
        for k in 0..n {
            let (a, b) = (k * len / n, (k + 1) * len / n);
            let gap = ((b - a) / 3).max(1);
            let (a, b) = (a + gap / 2, b - (gap - gap / 2));
            let r = if horizontal_line {
                Rect::new(a, cy - t / 2, b - a, t)
            } else {
                Rect::new(cx - t / 2, a, t, b - a)
            };
            fill(c, cell, clip, color, r);
        }
        return;
    }
    let vertical = width(up).max(width(down));
    let horizontal = width(left).max(width(right));
    // An arm from `from` to `to` along its axis; a double arm is two light lines.
    let mut arm = |weight: u8, horizontal_arm: bool, from: i32, to: i32| {
        if weight == NONE {
            return;
        }
        let t = width(if weight == DOUBLE { LIGHT } else { weight });
        let offsets: &[i32] = if weight == DOUBLE {
            &[-light, light]
        } else {
            &[0]
        };
        for &o in offsets {
            let r = if horizontal_arm {
                Rect::new(from, cy - t / 2 + o, to - from, t)
            } else {
                Rect::new(cx - t / 2 + o, from, t, to - from)
            };
            fill(c, cell, clip, color, r);
        }
    };
    let (hv, hh) = (vertical / 2 + 1, horizontal / 2 + 1);
    arm(up, false, 0, cy + hh);
    arm(down, false, cy - hh + 1, h);
    arm(left, true, 0, cx + hv);
    arm(right, true, cx - hv + 1, w);
}

/// A rounded corner: a quarter circle from the middle of one edge to the middle
/// of the next, of radius half the smaller side, then straight to the edges.
fn arc(c: &mut Canvas<'_>, clip: Rect, cell: Rect, code: u32, color: u32, t: i32) {
    let (w, h) = (cell.w as f32, cell.h as f32);
    let (cx, cy) = ((cell.w / 2) as f32 + 0.5, (cell.h / 2) as f32 + 0.5);
    let r = libm::floorf(w.min(h) / 2.0);
    // The circle's centre, toward the two arms: down-right, down-left, up-left,
    // up-right.
    let (sx, sy) = match code {
        0x256D => (1.0, 1.0),
        0x256E => (-1.0, 1.0),
        0x256F => (-1.0, -1.0),
        _ => (1.0, -1.0),
    };
    let (ox, oy) = (cx + sx * r, cy + sy * r);
    let half = t as f32 / 2.0;
    paint(c, cell, clip, color, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let (dx, dy) = (px - ox, py - oy);
        // On the circle's quarter facing the centre: an arc pixel.
        let in_quarter = dx * sx <= 0.0 && dy * sy <= 0.0;
        let d = if in_quarter {
            (libm::sqrtf(dx * dx + dy * dy) - r).abs()
        } else if dx * sx > 0.0 && dy * sy <= 0.0 {
            // Past the arc along x: the straight run to the edge on the arm's row.
            (py - cy).abs()
        } else if dy * sy > 0.0 && dx * sx <= 0.0 {
            (px - cx).abs()
        } else {
            f32::MAX
        };
        ((half + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u32
    });
    let _ = (w, h);
}

/// The diagonals: corner to corner, one or both.
fn diagonal(c: &mut Canvas<'_>, clip: Rect, cell: Rect, code: u32, color: u32, t: i32) {
    let (w, h) = (cell.w as f32, cell.h as f32);
    let len = libm::sqrtf(w * w + h * h);
    let half = t as f32 / 2.0;
    paint(c, cell, clip, color, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        // Distance to the line from top-right to bottom-left, and the other.
        let rising = (h * px + w * py - w * h).abs() / len;
        let falling = (h * px - w * py).abs() / len;
        let d = match code {
            0x2571 => rising,
            0x2572 => falling,
            _ => rising.min(falling),
        };
        ((half + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u32
    });
}

/// U+2580-259F.
fn blocks(c: &mut Canvas<'_>, clip: Rect, cell: Rect, code: u32, color: u32) {
    let (w, h) = (cell.w, cell.h);
    let eighth_h = |n: i32| h * n / 8;
    let eighth_w = |n: i32| w * n / 8;
    let (hw, hh) = (w / 2, h / 2);
    let quads = |ul: bool, ur: bool, ll: bool, lr: bool| {
        [
            (ul, Rect::new(0, 0, hw, hh)),
            (ur, Rect::new(hw, 0, w - hw, hh)),
            (ll, Rect::new(0, hh, hw, h - hh)),
            (lr, Rect::new(hw, hh, w - hw, h - hh)),
        ]
    };
    let mut rects: [(bool, Rect); 4] = [(false, Rect::default()); 4];
    match code {
        0x2580 => rects[0] = (true, Rect::new(0, 0, w, hh)),
        0x2581..=0x2588 => {
            let n = (code - 0x2580) as i32;
            rects[0] = (true, Rect::new(0, h - eighth_h(n), w, eighth_h(n)));
        }
        0x2589..=0x258F => {
            let n = (0x2590 - code) as i32;
            rects[0] = (true, Rect::new(0, 0, eighth_w(n), h));
        }
        0x2590 => rects[0] = (true, Rect::new(hw, 0, w - hw, h)),
        0x2591..=0x2593 => {
            let a = (code - 0x2590) * 64;
            paint(c, cell, clip, color, |_, _| a);
            return;
        }
        0x2594 => rects[0] = (true, Rect::new(0, 0, w, eighth_h(1))),
        0x2595 => rects[0] = (true, Rect::new(w - eighth_w(1), 0, eighth_w(1), h)),
        0x2596 => rects = quads(false, false, true, false),
        0x2597 => rects = quads(false, false, false, true),
        0x2598 => rects = quads(true, false, false, false),
        0x2599 => rects = quads(true, false, true, true),
        0x259A => rects = quads(true, false, false, true),
        0x259B => rects = quads(true, true, true, false),
        0x259C => rects = quads(true, true, false, true),
        0x259D => rects = quads(false, true, false, false),
        0x259E => rects = quads(false, true, true, false),
        _ => rects = quads(false, true, true, true),
    }
    for (on, r) in rects {
        if on {
            fill(c, cell, clip, color, r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Image;

    const W: i32 = 9;
    const H: i32 = 20;

    fn cell(ch: char) -> Image {
        let mut img = Image::new(W, H);
        let all = img.bounds();
        assert!(draw(&mut img.canvas(), all, all, ch, 0x00FF_FFFF));
        img
    }

    fn lit(img: &Image, x: i32, y: i32) -> bool {
        img.px[(y * img.w + x) as usize] & 0xFF > 0x80
    }

    #[test]
    fn every_character_in_the_range_draws_something() {
        for code in 0x2500..=0x259F {
            let ch = char::from_u32(code).unwrap();
            let img = cell(ch);
            assert!(img.px.iter().any(|&p| p != 0), "U+{code:04X} drew nothing");
        }
        assert!(!handles('a') && !handles('\u{25A0}'));
    }

    #[test]
    fn lines_reach_the_edges_they_name_and_no_others() {
        let h = cell('─');
        assert!(lit(&h, 0, H / 2) && lit(&h, W - 1, H / 2));
        assert!(!lit(&h, W / 2, 0) && !lit(&h, W / 2, H - 1));
        let v = cell('│');
        assert!(lit(&v, W / 2, 0) && lit(&v, W / 2, H - 1));
        let corner = cell('┌');
        assert!(lit(&corner, W - 1, H / 2) && lit(&corner, W / 2, H - 1));
        assert!(!lit(&corner, 0, H / 2) && !lit(&corner, W / 2, 0));
        let cross = cell('┼');
        for (x, y) in [
            (0, H / 2),
            (W - 1, H / 2),
            (W / 2, 0),
            (W / 2, H - 1),
            (W / 2, H / 2),
        ] {
            assert!(lit(&cross, x, y), "({x}, {y})");
        }
    }

    #[test]
    fn heavy_is_wider_than_light_and_double_is_two_lines() {
        let width = |img: &Image| (0..H).filter(|&y| lit(img, 0, y)).count();
        assert!(width(&cell('━')) > width(&cell('─')));
        let double: std::vec::Vec<bool> = (0..H).map(|y| lit(&cell('═'), 0, y)).collect();
        let runs = double.windows(2).filter(|p| !p[0] && p[1]).count();
        assert_eq!(runs, 2, "two separate lines: {double:?}");
    }

    #[test]
    fn blocks_and_shades_cover_what_they_say() {
        let full = cell('█');
        assert!(full.px.iter().all(|&p| p & 0xFF == 0xFF));
        let lower = cell('▄');
        assert!(!lit(&lower, W / 2, 1) && lit(&lower, W / 2, H - 2));
        let medium = cell('▒');
        assert_eq!(medium.px[0] & 0xFF, 128);
        let ul = cell('▘');
        assert!(lit(&ul, 1, 1) && !lit(&ul, W - 2, H - 2));
    }

    #[test]
    fn a_rounded_corner_meets_its_arms_at_the_edges() {
        let arc = cell('╭');
        assert!(lit(&arc, W - 1, H / 2), "right edge");
        assert!(lit(&arc, W / 2, H - 1), "bottom edge");
        assert!(!lit(&arc, 0, H / 2) && !lit(&arc, W / 2, 0));
    }
}

#[cfg(test)]
mod sheet {
    use super::*;
    use crate::canvas::Image;

    /// `cargo test -p gfx2d sheet -- --ignored` writes every character, 16 a
    /// row at 9x20 px, to target/gfx2d-boxdraw.ppm.
    #[test]
    #[ignore]
    fn write_the_sheet() {
        let (w, h) = (9, 20);
        let mut img = Image::new(16 * w, 10 * h);
        let all = img.bounds();
        img.canvas().fill(all, 0xFF14_1821);
        for code in 0x2500u32..=0x259F {
            let i = (code - 0x2500) as i32;
            let cell = Rect::new((i % 16) * w, (i / 16) * h, w, h);
            draw(
                &mut img.canvas(),
                all,
                cell,
                char::from_u32(code).unwrap(),
                0x00EC_E6DA,
            );
        }
        let mut out = std::format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
        for p in &img.px {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gfx2d-boxdraw.ppm"),
            out,
        )
        .unwrap();
    }
}
