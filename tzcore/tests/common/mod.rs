//! A TZif writer for the tests (RFC 8536 version 2, written as `zic -b slim`
//! writes it: a minimal 32-bit block), and the zones the tests build with it
//! from published transition dates. No tz data is copied here.

#![allow(dead_code)]

use tzcore::{DateTime, Zone};

/// A local time type: offset east of UTC in seconds, DST or not, abbreviation.
pub type Kind = (i32, bool, &'static str);

/// Seconds from the epoch of `y`-`m`-`d` `h`:`min` UTC.
pub fn utc(y: i32, m: u8, d: u8, h: u8, min: u8) -> i64 {
    DateTime::date(y, m, d).at(h, min, 0).seconds()
}

/// The bytes of a TZif version 2 file.
pub fn tzif(transitions: &[(i64, u8)], kinds: &[Kind], footer: &str) -> Vec<u8> {
    let mut out = Vec::new();
    // The 32-bit block: one type and one NUL, as slim files have.
    header(&mut out, [0, 0, 0, 0, 1, 1]);
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]);
    let mut chars: Vec<u8> = Vec::new();
    let mut at = Vec::new();
    for &(_, _, abbr) in kinds {
        at.push(chars.len() as u8);
        chars.extend_from_slice(abbr.as_bytes());
        chars.push(0);
    }
    let counts = [
        0,
        0,
        0,
        transitions.len() as u32,
        kinds.len() as u32,
        chars.len() as u32,
    ];
    header(&mut out, counts);
    for &(time, _) in transitions {
        out.extend_from_slice(&time.to_be_bytes());
    }
    out.extend(transitions.iter().map(|t| t.1));
    for (i, &(offset, dst, _)) in kinds.iter().enumerate() {
        out.extend_from_slice(&offset.to_be_bytes());
        out.push(u8::from(dst));
        out.push(at[i]);
    }
    out.extend_from_slice(&chars);
    out.push(b'\n');
    out.extend_from_slice(footer.as_bytes());
    out.push(b'\n');
    out
}

fn header(out: &mut Vec<u8>, counts: [u32; 6]) {
    out.extend_from_slice(b"TZif2");
    out.extend_from_slice(&[0; 15]);
    for n in counts {
        out.extend_from_slice(&n.to_be_bytes());
    }
}

const EST: Kind = (-5 * 3600, false, "EST");
const EDT: Kind = (-4 * 3600, true, "EDT");

/// America/New_York's shape: 2006 under the old US rule (first Sunday of
/// April to last Sunday of October), 2007 under the new one, and the new one
/// as its footer. As `zic` writes it, the footer agrees with the last
/// transition listed.
pub fn new_york() -> Zone {
    let transitions = [
        (utc(2006, 4, 2, 7, 0), 1),
        (utc(2006, 10, 29, 6, 0), 0),
        (utc(2007, 3, 11, 7, 0), 1),
        (utc(2007, 11, 4, 6, 0), 0),
    ];
    Zone::parse(&tzif(&transitions, &[EST, EDT], "EST5EDT,M3.2.0,M11.1.0")).unwrap()
}

/// Europe/Berlin's rule since 1996, alone.
pub fn berlin() -> Zone {
    Zone::parse(&tzif(
        &[],
        &[(3600, false, "CET")],
        "CET-1CEST,M3.5.0,M10.5.0/3",
    ))
    .unwrap()
}

/// America/Sao_Paulo's last daylight saving time, 2018-11-04 to 2019-02-17;
/// Brazil dropped it after (decree 9,772 of 2019), so its footer has none.
pub fn sao_paulo() -> Zone {
    let kinds = [(-3 * 3600, false, "-03"), (-2 * 3600, true, "-02")];
    let transitions = [(utc(2018, 11, 4, 3, 0), 1), (utc(2019, 2, 17, 2, 0), 0)];
    Zone::parse(&tzif(&transitions, &kinds, "<-03>3")).unwrap()
}

/// Australia/Sydney's rule since 2008: first Sunday of October to first
/// Sunday of April.
pub fn sydney() -> Zone {
    Zone::parse(&tzif(
        &[],
        &[(10 * 3600, false, "AEST")],
        "AEST-10AEDT,M10.1.0,M4.1.0/3",
    ))
    .unwrap()
}

/// Australia/Adelaide: Sydney's dates, half an hour behind.
pub fn adelaide() -> Zone {
    let footer = "ACST-9:30ACDT,M10.1.0,M4.1.0/3";
    Zone::parse(&tzif(&[], &[(9 * 3600 + 1800, false, "ACST")], footer)).unwrap()
}

/// Asia/Kathmandu: +05:30 until 1986, then +05:45.
pub fn kathmandu() -> Zone {
    let kinds = [
        (5 * 3600 + 1800, false, "+0530"),
        (5 * 3600 + 2700, false, "+0545"),
    ];
    let transitions = [(utc(1985, 12, 31, 18, 30), 1)];
    Zone::parse(&tzif(&transitions, &kinds, "<+0545>-5:45")).unwrap()
}
