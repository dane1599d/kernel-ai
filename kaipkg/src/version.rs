//! Versions and ranges (ADR 0011 §2): SemVer `MAJOR.MINOR.PATCH` with no
//! pre-release or build tags in format 1, and ranges written `^`, `~`, `=` or as
//! one or two comparisons (`>=1.2.0 <2.0.0`), with cargo's meanings.

use alloc::vec::Vec;
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

/// One number of a version (or a generation): digits, no sign, no leading zero.
pub(crate) fn number(text: &str) -> Result<u64, &'static str> {
    let plain = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if !plain {
        return Err("a version number is not plain digits");
    }
    text.parse().map_err(|_| "a version number is too large")
}

/// One to three numbers, for the ranges' partial versions.
fn numbers(text: &str) -> Result<Vec<u64>, &'static str> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() > 3 {
        return Err("a version has more than three numbers");
    }
    parts.into_iter().map(number).collect()
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// `MAJOR.MINOR.PATCH`, nothing else.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        match numbers(text)?.as_slice() {
            &[major, minor, patch] => Ok(Self::new(major, minor, patch)),
            _ => Err("a version is not MAJOR.MINOR.PATCH"),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A range: every version at or above `low` and below `high` (None: no bound).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub low: Version,
    pub high: Option<Version>,
}

impl Range {
    pub fn matches(&self, v: &Version) -> bool {
        *v >= self.low && self.high.is_none_or(|high| *v < high)
    }

    /// `^x.y.z`, `~x.y.z`, `=x.y.z` (the first two take partial versions), or one
    /// or two of `>=`, `>`, `<=` and `<` with full versions, space-separated.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let words: Vec<&str> = text.split(' ').collect();
        if let [word] = words.as_slice()
            && let Some(op) = word.chars().next().filter(|c| matches!(c, '^' | '~' | '='))
        {
            return Self::unary(op, &word[1..]);
        }
        if words.is_empty() || words.len() > 2 {
            return Err("a range is not one operator and a version, or two comparisons");
        }
        let mut range = Range {
            low: Version::new(0, 0, 0),
            high: None,
        };
        let (mut low_set, mut high_set) = (false, false);
        for word in words {
            let (op, rest) = [">=", "<=", ">", "<"]
                .into_iter()
                .find_map(|op| word.strip_prefix(op).map(|rest| (op, rest)))
                .ok_or("a comparison has no >=, >, <= or <")?;
            let v = Version::parse(rest)?;
            let next = |v: Version| Version::new(v.major, v.minor, v.patch.saturating_add(1));
            match op {
                ">=" | ">" if !low_set => {
                    range.low = if op == ">" { next(v) } else { v };
                    low_set = true;
                }
                "<=" | "<" if !high_set => {
                    range.high = Some(if op == "<=" { next(v) } else { v });
                    high_set = true;
                }
                _ => return Err("a range bounds the same side twice"),
            }
        }
        Ok(range)
    }

    fn unary(op: char, version: &str) -> Result<Self, &'static str> {
        let n = numbers(version)?;
        let at = |i: usize| n.get(i).copied().unwrap_or(0);
        let low = Version::new(at(0), at(1), at(2));
        let bump = |major: u64, minor: u64, patch: u64| {
            (major != u64::MAX).then(|| Version::new(major, minor, patch))
        };
        let high = match (op, n.len()) {
            ('=', 3) => bump(low.major, low.minor, low.patch.saturating_add(1)),
            ('=', _) => return Err("an = range needs MAJOR.MINOR.PATCH"),
            ('~', 1) | ('^', 1) => bump(low.major.saturating_add(1), 0, 0),
            ('~', _) => bump(low.major, low.minor.saturating_add(1), 0),
            // Caret: the leftmost non-zero number stays.
            (_, _) if low.major > 0 => bump(low.major + 1, 0, 0),
            (_, 2) => bump(0, low.minor.saturating_add(1), 0),
            (_, _) if low.minor > 0 => bump(0, low.minor + 1, 0),
            (_, _) => bump(0, 0, low.patch.saturating_add(1)),
        };
        Ok(Range { low, high })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn admits(range: &str, yes: &[&str], no: &[&str]) {
        let r = Range::parse(range).unwrap();
        for y in yes {
            assert!(r.matches(&v(y)), "{range} should admit {y}");
        }
        for n in no {
            assert!(!r.matches(&v(n)), "{range} should refuse {n}");
        }
    }

    #[test]
    fn versions_are_strict() {
        assert_eq!(v("2.1.0"), Version::new(2, 1, 0));
        for bad in [
            "2.1",
            "2.1.0.0",
            "02.1.0",
            "2.1.0-rc1",
            "2.1.0+b",
            "v2.1.0",
            "",
            "1..2",
            "+1.0.0",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
        assert!(v("1.10.0") > v("1.9.9"));
    }

    #[test]
    fn ranges_mean_what_cargo_means() {
        admits("^1.2", &["1.2.0", "1.9.9"], &["1.1.9", "2.0.0"]);
        admits("^1.2.3", &["1.2.3", "1.3.0"], &["1.2.2", "2.0.0"]);
        admits("^0.2.3", &["0.2.3", "0.2.9"], &["0.3.0", "0.2.2"]);
        admits("^0.0.3", &["0.0.3"], &["0.0.4", "0.0.2"]);
        admits("^0", &["0.0.0", "0.9.9"], &["1.0.0"]);
        admits("~1.2.3", &["1.2.3", "1.2.9"], &["1.3.0"]);
        admits("~1", &["1.0.0", "1.9.0"], &["2.0.0"]);
        admits("=1.2.3", &["1.2.3"], &["1.2.4", "1.2.2"]);
        admits(">=1.2.0 <2.0.0", &["1.2.0", "1.99.0"], &["2.0.0", "1.1.0"]);
        admits(">1.2.0 <=1.3.0", &["1.2.1", "1.3.0"], &["1.2.0", "1.3.1"]);
        admits(">=3.0.0", &["3.0.0", "99.0.0"], &["2.9.9"]);
    }

    #[test]
    fn malformed_ranges_are_refused() {
        for bad in [
            "",
            "1.2",
            "^",
            "^1.2.3.4",
            "=1.2",
            ">=1.2",
            ">=1.0.0 >=2.0.0",
            "<1.0.0 <2.0.0",
            ">=1.0.0 <2.0.0 <3.0.0",
            "^1.2 ^1.3",
            "!=1.0.0",
            "^ 1.2",
            "^01.2",
        ] {
            assert!(Range::parse(bad).is_err(), "{bad:?}");
        }
    }
}
