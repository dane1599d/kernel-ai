//! The console's rows for the terminal window (P3 task 8): what TERM_READ
//! copies out, and how the window reads it back.
//!
//! - The kernel keeps a [`Log`] beside the grid: a sequence number for each row,
//!   bumped when the row changes, and a count of the rows scrolled. It is kept
//!   apart from the console's paint damage, so a reader never takes damage the
//!   console still has to paint, and each reader asks with its own `since`.
//! - [`Log::snapshot`] writes the rows changed after `since`, with their kinds,
//!   the cursor and the scroll count, into a buffer reserved before the console's
//!   lock is taken: it never allocates. Only the grid's rows go out; nothing else
//!   the kernel keeps.
//! - [`Snapshot::decode`] reads it back. A reader that has seen `scrolls` rows
//!   scrolled moves its picture up by the difference, then repaints the rows.
//!
//! On the wire, little-endian: a [`HEADER`]-byte header (`seq` u64, `scrolls`
//! u64, `cols` u16, `rows` u16, the cursor's row and column as u16, `0xFFFF`
//! for none, the row count u16, 6 zero bytes), then each row: its index u16, its
//! kind's tag u8, a zero byte, the kind's number u16 (a `tag:` or prompt width),
//! the text's length in bytes u16, then the text in UTF-8.

use crate::grid::{Damage, Grid, MAX_ROWS};
use crate::style::Kind;
use alloc::vec::Vec;

/// Bytes before the rows.
pub const HEADER: usize = 32;
/// Bytes before each row's text.
pub const ROW_HEADER: usize = 8;
/// Cells the console's grid holds (`kernel/src/console`'s `CELLS`).
pub const CELLS: usize = 32 * 1024;
/// The most a snapshot of a [`CELLS`]-cell grid takes: every row, every cell 4
/// bytes of UTF-8.
pub const MAX_BYTES: usize = HEADER + MAX_ROWS * ROW_HEADER + CELLS * 4;
const NONE: u16 = u16::MAX;

/// Each row's sequence number, and the rows scrolled, since the grid began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Log {
    seq: u64,
    scrolls: u64,
    rows: [u64; MAX_ROWS],
}

impl Default for Log {
    fn default() -> Self {
        Self::new()
    }
}

/// The snapshot did not fit the buffer reserved for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Full;

/// The bytes were not a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed;

impl Log {
    /// Every row as changed at sequence 1, so a first read (`since` 0) gets all.
    pub const fn new() -> Self {
        Self {
            seq: 1,
            scrolls: 0,
            rows: [1; MAX_ROWS],
        }
    }

    /// The latest sequence number.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Takes what the grid's `damage` says changed in its first `rows` rows.
    pub fn fold(&mut self, damage: Damage, rows: usize) {
        let rows = rows.min(MAX_ROWS);
        if damage.rows == 0 && damage.scrolled == 0 {
            return;
        }
        self.seq += 1;
        if damage.scrolled > 0 {
            self.scrolls += damage.scrolled as u64;
            for row in 0..rows {
                self.rows[row] = match row + damage.scrolled {
                    from if from < rows => self.rows[from],
                    _ => self.seq,
                };
            }
        }
        for row in (0..rows).filter(|&row| damage.has(row)) {
            self.rows[row] = self.seq;
        }
    }

    /// Every row changed: the grid was resized or cleared.
    pub fn touch_all(&mut self) {
        self.seq += 1;
        self.rows = [self.seq; MAX_ROWS];
    }

    /// Writes the rows of `grid` changed after `since` into `out`, which must
    /// have room reserved: it never grows `out`, and leaves it as it was on
    /// [`Full`].
    pub fn snapshot<const N: usize>(
        &self,
        grid: &Grid<N>,
        since: u64,
        out: &mut Vec<u8>,
    ) -> Result<(), Full> {
        let start = out.len();
        let result = self.write(grid, since, out);
        if result.is_err() {
            out.truncate(start);
        }
        result
    }

    fn write<const N: usize>(
        &self,
        grid: &Grid<N>,
        since: u64,
        out: &mut Vec<u8>,
    ) -> Result<(), Full> {
        let (cols, rows) = grid.size();
        let rows = rows.min(MAX_ROWS);
        let count = (0..rows).filter(|&row| self.rows[row] > since).count();
        let (cursor_row, cursor_col) = grid
            .cursor()
            .map_or((NONE, NONE), |(row, col)| (row as u16, col as u16));
        put(out, &self.seq.to_le_bytes())?;
        put(out, &self.scrolls.to_le_bytes())?;
        for word in [
            cols as u16,
            rows as u16,
            cursor_row,
            cursor_col,
            count as u16,
        ] {
            put(out, &word.to_le_bytes())?;
        }
        put(out, &[0; 6])?;
        let input = grid.input_span();
        for row in (0..rows).filter(|&row| self.rows[row] > since) {
            let cells = grid.row(row);
            let kind = match input.filter(|&(top, end, _)| (top..end).contains(&row)) {
                Some((top, _, prompt)) if row == top => Kind::Input { prompt },
                Some(_) => Kind::Input { prompt: 0 },
                None => Kind::of(cells),
            };
            let width = grid.row_width(row);
            let bytes: usize = cells[..width].iter().map(|ch| ch.len_utf8()).sum();
            let (tag, number) = encode_kind(kind);
            put(out, &(row as u16).to_le_bytes())?;
            put(out, &[tag, 0])?;
            put(out, &number.to_le_bytes())?;
            put(out, &(bytes as u16).to_le_bytes())?;
            for ch in &cells[..width] {
                let mut utf8 = [0u8; 4];
                put(out, ch.encode_utf8(&mut utf8).as_bytes())?;
            }
        }
        Ok(())
    }
}

/// Appends `bytes` if the room reserved holds them.
fn put(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Full> {
    if out.capacity() - out.len() < bytes.len() {
        return Err(Full);
    }
    out.extend_from_slice(bytes);
    Ok(())
}

fn encode_kind(kind: Kind) -> (u8, u16) {
    match kind {
        Kind::Log { tag } => (0, tag as u16),
        Kind::Banner => (1, 0),
        Kind::Input { prompt } => (2, prompt as u16),
        Kind::Reply => (3, 0),
        Kind::Ai => (4, 0),
        Kind::Error => (5, 0),
        Kind::Question => (6, 0),
        Kind::Success => (7, 0),
    }
}

fn decode_kind(tag: u8, number: u16) -> Option<Kind> {
    let number = usize::from(number);
    Some(match tag {
        0 => Kind::Log { tag: number },
        1 => Kind::Banner,
        2 => Kind::Input { prompt: number },
        3 => Kind::Reply,
        4 => Kind::Ai,
        5 => Kind::Error,
        6 => Kind::Question,
        7 => Kind::Success,
        _ => return None,
    })
}

/// A snapshot, read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot<'a> {
    /// Pass this as the next read's `since`.
    pub seq: u64,
    /// Rows scrolled since the grid began.
    pub scrolls: u64,
    pub cols: usize,
    pub rows: usize,
    /// The cursor's (row, column), if it shows.
    pub cursor: Option<(usize, usize)>,
    count: usize,
    body: &'a [u8],
}

/// One changed row: its index, its kind and its text, trailing blanks dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row<'a> {
    pub row: usize,
    pub kind: Kind,
    pub text: &'a str,
}

impl<'a> Snapshot<'a> {
    /// Reads `bytes`, checking every row, so [`Snapshot::rows`] cannot fail.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Malformed> {
        let header = bytes.get(..HEADER).ok_or(Malformed)?;
        let u64_at =
            |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap_or([0; 8]));
        let u16_at = |at: usize| usize::from(u16::from_le_bytes([header[at], header[at + 1]]));
        let (cols, rows) = (u16_at(16), u16_at(18));
        let cursor = match (u16_at(20), u16_at(22)) {
            (row, col) if row == usize::from(NONE) || col == usize::from(NONE) => None,
            (row, col) if row < rows && col < cols => Some((row, col)),
            _ => return Err(Malformed),
        };
        if rows > MAX_ROWS || header[26..32].iter().any(|&b| b != 0) {
            return Err(Malformed);
        }
        let snapshot = Self {
            seq: u64_at(0),
            scrolls: u64_at(8),
            cols,
            rows,
            cursor,
            count: u16_at(24),
            body: &bytes[HEADER..],
        };
        let mut seen = 0;
        let mut rest = snapshot.body;
        while !rest.is_empty() {
            let (row, next) = read_row(rest).ok_or(Malformed)?;
            if row.row >= rows || row.text.chars().count() > cols {
                return Err(Malformed);
            }
            rest = next;
            seen += 1;
        }
        match seen == snapshot.count {
            true => Ok(snapshot),
            false => Err(Malformed),
        }
    }

    /// The changed rows, in order.
    pub fn rows(&self) -> impl Iterator<Item = Row<'a>> + 'a {
        let mut rest = self.body;
        core::iter::from_fn(move || {
            let (row, next) = read_row(rest)?;
            rest = next;
            Some(row)
        })
    }
}

fn read_row(bytes: &[u8]) -> Option<(Row<'_>, &[u8])> {
    let head = bytes.get(..ROW_HEADER)?;
    let u16_at = |at: usize| u16::from_le_bytes([head[at], head[at + 1]]);
    if head[3] != 0 {
        return None;
    }
    let len = usize::from(u16_at(6));
    let text = bytes.get(ROW_HEADER..ROW_HEADER + len)?;
    let row = Row {
        row: usize::from(u16_at(0)),
        kind: decode_kind(head[2], u16_at(4))?,
        text: core::str::from_utf8(text).ok()?,
    };
    Some((row, &bytes[ROW_HEADER + len..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    type Small = Grid<{ 20 * 4 }>;

    fn grid() -> Small {
        let mut g = Small::new();
        g.resize(20, 4);
        g
    }

    /// The snapshot of `g` after `since`, as (row, kind, text) rows.
    fn read(log: &Log, g: &Small, since: u64) -> (u64, u64, Vec<(usize, Kind, String)>) {
        let mut out = Vec::with_capacity(MAX_BYTES);
        log.snapshot(g, since, &mut out).unwrap();
        let snapshot = Snapshot::decode(&out).unwrap();
        let rows = snapshot
            .rows()
            .map(|r| (r.row, r.kind, String::from(r.text)))
            .collect();
        (snapshot.seq, snapshot.scrolls, rows)
    }

    #[test]
    fn a_first_read_gets_every_row_and_later_reads_only_changes() {
        let (mut g, mut log) = (grid(), Log::new());
        g.log("= one\n");
        log.fold(g.take_damage(), 4);
        let (seq, scrolls, rows) = read(&log, &g, 0);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], (0, Kind::Reply, String::from("= one")));
        assert_eq!(scrolls, 0);
        assert_eq!(read(&log, &g, seq).2, []);
        g.log("! two\n");
        log.fold(g.take_damage(), 4);
        let (_, _, rows) = read(&log, &g, seq);
        assert_eq!(rows, [(1, Kind::Error, String::from("! two"))]);
    }

    #[test]
    fn a_scroll_is_counted_and_moved_rows_keep_their_sequence() {
        let (mut g, mut log) = (grid(), Log::new());
        for line in ["a", "b", "c", "d"] {
            g.log(line);
            g.log("\n");
        }
        log.fold(g.take_damage(), 4);
        let (seq, before, _) = read(&log, &g, 0);
        g.log("e\n");
        log.fold(g.take_damage(), 4);
        let (_, after, rows) = read(&log, &g, seq);
        assert!(after > before);
        // What scrolled up is not sent again; only rows that changed are.
        assert!(rows.iter().all(|(row, ..)| *row >= 2), "{rows:?}");
        assert!(rows.iter().any(|(_, _, text)| text == "e"), "{rows:?}");
    }

    #[test]
    fn the_input_row_and_cursor_go_with_it() {
        let (mut g, mut log) = (grid(), Log::new());
        g.set_input("kai> ", "pol", 3);
        log.fold(g.take_damage(), 4);
        let mut out = Vec::with_capacity(MAX_BYTES);
        log.snapshot(&g, 0, &mut out).unwrap();
        let snapshot = Snapshot::decode(&out).unwrap();
        let input = snapshot.rows().find(|r| r.text == "kai> pol").unwrap();
        assert_eq!(input.kind, Kind::Input { prompt: 5 });
        assert_eq!(snapshot.cursor, Some((input.row, 8)));
    }

    #[test]
    fn it_never_grows_the_buffer_and_leaves_it_whole_when_full() {
        let (mut g, mut log) = (grid(), Log::new());
        g.log("= a row of text\n");
        log.fold(g.take_damage(), 4);
        let mut out = Vec::with_capacity(HEADER + 4);
        out.push(9);
        assert_eq!(log.snapshot(&g, 0, &mut out), Err(Full));
        assert_eq!(out, [9]);
        assert_eq!(out.capacity(), HEADER + 4);
    }

    #[test]
    fn bad_bytes_are_refused_never_misread() {
        let (mut g, mut log) = (grid(), Log::new());
        g.log("? Stop \"b\"?\n");
        log.fold(g.take_damage(), 4);
        let mut out = Vec::with_capacity(MAX_BYTES);
        log.snapshot(&g, 0, &mut out).unwrap();
        for cut in 0..out.len() {
            assert_eq!(
                Snapshot::decode(&out[..cut]),
                Err(Malformed),
                "cut at {cut}"
            );
        }
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..2000 {
            let mut bytes = out.clone();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = seed as usize % bytes.len();
            bytes[at] = (seed >> 40) as u8;
            if let Ok(snapshot) = Snapshot::decode(&bytes) {
                for row in snapshot.rows() {
                    assert!(row.row < snapshot.rows);
                }
            }
        }
        assert_eq!(Snapshot::decode(&[0; HEADER + 1]), Err(Malformed));
    }
}
