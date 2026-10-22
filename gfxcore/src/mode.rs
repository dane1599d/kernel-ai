//! Which mode the display runs in: `display=WIDTHxHEIGHT` from the command line
//! when it is in range, otherwise the largest sensible mode the device offers.

use crate::edid::Mode;

/// The smallest mode the kernel runs: below it the console has too few columns.
pub const MIN: Mode = (640, 480);
/// The largest mode picked unasked. Larger panels cost more per full-screen
/// flush, and the console's grid stops growing near here (32 Ki cells).
pub const AUTO_MAX: Mode = (1920, 1200);
/// The largest mode `display=` may ask for (4K UHD: 32 MiB of back buffer).
pub const MAX: Mode = (3840, 2160);
/// Used when the device offers nothing in range.
pub const FALLBACK: Mode = (1024, 768);

/// `display=WIDTHxHEIGHT` from `cmdline`, whether or not it is in range.
pub fn requested(cmdline: &str) -> Option<Mode> {
    let value = cmdline
        .split_whitespace()
        .find_map(|arg| arg.strip_prefix("display="))?;
    let (width, height) = value.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

pub fn in_range(mode: Mode, max: Mode) -> bool {
    (MIN.0..=max.0).contains(&mode.0) && (MIN.1..=max.1).contains(&mode.1)
}

/// The mode to set: `requested` when it is within [`MIN`]..[`MAX`], otherwise the
/// largest of `offered` (by area, then width) within [`MIN`]..[`AUTO_MAX`], or
/// [`FALLBACK`] when none is.
pub fn choose(offered: &[Mode], requested: Option<Mode>) -> Mode {
    if let Some(mode) = requested.filter(|&mode| in_range(mode, MAX)) {
        return mode;
    }
    offered
        .iter()
        .copied()
        .filter(|&mode| in_range(mode, AUTO_MAX))
        .max_by_key(|&(width, height)| (u64::from(width) * u64::from(height), width))
        .unwrap_or(FALLBACK)
}

#[cfg(test)]
mod tests {
    use super::{FALLBACK, choose, requested};
    use crate::edid::{modes, tests::sample};

    #[test]
    fn picks_the_largest_offered_mode_up_to_1920x1200() {
        assert_eq!(choose(&modes(&sample()), None), (1920, 1080));
        assert_eq!(choose(&[(1280, 800), (2560, 1600)], None), (1280, 800));
        assert_eq!(choose(&[(1600, 1200), (1920, 1080)], None), (1920, 1080));
        assert_eq!(choose(&[(1920, 1200), (1920, 1080)], None), (1920, 1200));
    }

    #[test]
    fn falls_back_when_nothing_is_in_range() {
        assert_eq!(choose(&[], None), FALLBACK);
        assert_eq!(choose(&[(320, 200), (4096, 2160)], None), FALLBACK);
    }

    #[test]
    fn display_argument_wins_when_in_range() {
        let offered = [(1280, 800)];
        assert_eq!(
            choose(&offered, requested("test display=1024x768")),
            (1024, 768)
        );
        assert_eq!(
            choose(&offered, requested("display=3840x2160")),
            (3840, 2160)
        );
        assert_eq!(
            choose(&offered, requested("display=8000x8000")),
            (1280, 800)
        );
        assert_eq!(choose(&offered, requested("display=100x100")), (1280, 800));
    }

    #[test]
    fn display_argument_must_be_two_numbers() {
        assert_eq!(requested("display=1920x1080"), Some((1920, 1080)));
        assert_eq!(requested("a display=800x600 b"), Some((800, 600)));
        assert_eq!(requested("display=1920"), None);
        assert_eq!(requested("display=axb"), None);
        assert_eq!(requested("nodisplay=1x1"), None);
    }
}
