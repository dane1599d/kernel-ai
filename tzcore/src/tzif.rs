//! TZif, the compiled form of the tz database (RFC 8536): what `zic` writes and
//! `/usr/share/zoneinfo` holds. Versions 1 to 4 are read. From version 2 on,
//! the first (32-bit) block is skipped for the second (64-bit) one and the
//! footer's TZ string, which gives the rule after the last transition. Leap
//! seconds are skipped: a zone's times are POSIX times, as `zic` writes them
//! by default.

use crate::rule::{Kind, Rule};
use crate::{Error, Zone};
use alloc::string::String;
use alloc::vec::Vec;

/// The largest file read: the biggest zone in the database is under 4 KiB.
pub const MAX_BYTES: usize = 64 * 1024;

/// The six counts of a header, in file order.
struct Counts {
    isut: usize,
    isstd: usize,
    leap: usize,
    time: usize,
    kinds: usize,
    chars: usize,
}

impl Counts {
    /// The data block's length with `time_size`-byte times.
    fn block(&self, time_size: usize) -> Option<usize> {
        let parts = [
            self.time.checked_mul(time_size)?,
            self.time,
            self.kinds.checked_mul(6)?,
            self.chars,
            self.leap.checked_mul(time_size + 4)?,
            self.isstd,
            self.isut,
        ];
        parts
            .iter()
            .try_fold(0usize, |sum, part| sum.checked_add(*part))
    }
}

struct Reader<'a> {
    s: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Truncated)?;
        let part = self.s.get(self.at..end).ok_or(Error::Truncated)?;
        self.at = end;
        Ok(part)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn header(&mut self) -> Result<(u8, Counts), Error> {
        if self.take(4)? != b"TZif" {
            return Err(Error::Magic);
        }
        let version = match self.take(1)?[0] {
            0 => 1,
            v @ b'2'..=b'4' => v - b'0',
            _ => return Err(Error::Version),
        };
        self.take(15)?;
        let mut count = || self.u32().map(|n| n as usize);
        let counts = Counts {
            isut: count()?,
            isstd: count()?,
            leap: count()?,
            time: count()?,
            kinds: count()?,
            chars: count()?,
        };
        let ok = counts.kinds >= 1
            && counts.kinds <= 256
            && counts.chars >= 1
            && (counts.isut == 0 || counts.isut == counts.kinds)
            && (counts.isstd == 0 || counts.isstd == counts.kinds);
        if !ok {
            return Err(Error::Counts);
        }
        Ok((version, counts))
    }
}

/// Parses a TZif file.
pub fn parse(bytes: &[u8]) -> Result<Zone, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooBig);
    }
    let mut r = Reader { s: bytes, at: 0 };
    let (version, v1) = r.header()?;
    let v1_len = v1.block(4).ok_or(Error::Counts)?;
    if version == 1 {
        return block(&mut r, &v1, 4, None);
    }
    // The 32-bit block holds the same zone, cut to 1901-2038: skipped.
    r.take(v1_len)?;
    let (_, v2) = r.header()?;
    let start = r.at;
    r.take(v2.block(8).ok_or(Error::Counts)?)?;
    let footer = footer(&mut r)?;
    r.at = start;
    block(&mut r, &v2, 8, footer)
}

/// The footer, `\n<TZ string>\n`; an empty string means none.
fn footer(r: &mut Reader<'_>) -> Result<Option<Rule>, Error> {
    let rest = &r.s[r.at..];
    let text = rest
        .strip_prefix(b"\n")
        .and_then(|t| t.strip_suffix(b"\n"))
        .ok_or(Error::Footer)?;
    if text.contains(&b'\n') {
        return Err(Error::Footer);
    }
    if text.is_empty() {
        return Ok(None);
    }
    let text = core::str::from_utf8(text).map_err(|_| Error::Footer)?;
    Rule::parse(text).map(Some)
}

/// One data block, with `size`-byte transition times.
fn block(r: &mut Reader<'_>, c: &Counts, size: usize, rule: Option<Rule>) -> Result<Zone, Error> {
    // `parse` checked the whole block's length, so none of these overflow.
    let times = r.take(c.time * size)?;
    let times: Vec<i64> = times
        .chunks_exact(size)
        .map(|b| match size {
            4 => i64::from(i32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            _ => i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        })
        .collect();
    if times.windows(2).any(|w| w[0] >= w[1]) {
        return Err(Error::Order);
    }
    let indices = r.take(c.time)?.to_vec();
    if indices.iter().any(|&i| usize::from(i) >= c.kinds) {
        return Err(Error::Index);
    }
    let raw = r.take(c.kinds * 6)?;
    let chars = r.take(c.chars)?;
    let mut kinds = Vec::with_capacity(c.kinds);
    for t in raw.as_chunks::<6>().0 {
        let offset = i32::from_be_bytes([t[0], t[1], t[2], t[3]]);
        // RFC 8536 §3.2: -2^31 is not allowed, and no zone is a day off UTC.
        if !(-89_999..=93_599).contains(&offset) || t[4] > 1 {
            return Err(Error::Kind);
        }
        let abbr = abbreviation(chars, usize::from(t[5]))?;
        kinds.push(Kind {
            offset,
            dst: t[4] == 1,
            abbr,
        });
    }
    Ok(Zone {
        times,
        indices,
        kinds,
        rule,
    })
}

/// The NUL-terminated abbreviation at `at` in the designation bytes.
fn abbreviation(chars: &[u8], at: usize) -> Result<String, Error> {
    let rest = chars.get(at..).ok_or(Error::Kind)?;
    let end = rest.iter().position(|&b| b == 0).ok_or(Error::Kind)?;
    let text = core::str::from_utf8(&rest[..end]).map_err(|_| Error::Kind)?;
    Ok(String::from(text))
}
