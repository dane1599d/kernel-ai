//! Hostile catalogues (P4 task 3, the P4 plan's host gates): whatever the Apps
//! sheet is handed, `kaipkg::catalog::parse` refuses it or reads rows that
//! write back as the same bytes, each summary already as safe to draw as
//! `summary_of` makes one, and never more than its row cap.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kaipkg::catalog::{MAX_ROWS, MAX_SUMMARY, format, parse, summary_of};

const SAMPLE: &str = "kai-catalog 1\n\
    installed\tnotes\t1.0.0\tapp\t74510\tMPL-2.0\t-\t-\tPlain-text notes the AI can read and write\n\
    mirror\tbig\t1.2.0\tdata\t307300\tMIT OR Apache-2.0\tfiles,net:dns\t1.0.0\tA test package\n";

fn reads_back(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let Ok(rows) = parse(text) else {
        return;
    };
    assert!(rows.len() <= MAX_ROWS);
    for row in &rows {
        assert_eq!(row.summary, summary_of(&row.summary), "{text:?}");
        assert!(row.summary.len() <= MAX_SUMMARY);
    }
    assert_eq!(format(&rows), text, "a parsed answer writes back as itself");
}

#[test]
fn mutated_catalogues_are_refused_or_read_back_exactly() {
    assert_eq!(parse(SAMPLE).map(|rows| rows.len()), Ok(2));
    hostile::fuzz(SAMPLE.as_bytes(), 0x6361, 5_000, reads_back);
    hostile::truncations(SAMPLE.as_bytes(), |_, cut| reads_back(cut));
}
