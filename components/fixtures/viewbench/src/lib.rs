//! S03-1's view and layout guest, bound by hand to the canonical ABI for
//! `viewbench.wit`. All memory is one bump arena, emptied by each export's
//! `cabi_post_*`.

#![cfg_attr(target_family = "wasm", no_std)]

/// Nodes in one view: the root, a header, 297 rows and a footer.
pub const NODES: usize = 300;
pub const NOTES: usize = 1000;
/// Bytes of text the layout pass wraps.
pub const TEXT_BYTES: usize = 8 << 10;

const WORDS: [&str; 16] = [
    "milk", "call", "bank", "about", "the", "meeting", "tomorrow", "kernel", "notes", "buy",
    "remember", "garden", "letter", "train", "ticket", "review",
];

/// A deterministic word source.
struct Words(u64);

impl Words {
    fn next_word(&mut self) -> &'static str {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        WORDS[(self.0 % WORDS.len() as u64) as usize]
    }
}

/// Note `n`'s text, written into `out`; returns its length.
pub fn note_text(n: usize, out: &mut [u8]) -> usize {
    let mut words = Words(0x9E37_79B9 + n as u64);
    let mut len = 0;
    for i in 0..3 + n % 9 {
        let word = words.next_word().as_bytes();
        if i > 0 && len < out.len() {
            out[len] = b' ';
            len += 1;
        }
        let take = word.len().min(out.len() - len);
        out[len..len + take].copy_from_slice(&word[..take]);
        len += take;
    }
    len
}

/// The layout pass's text: words and spaces, `TEXT_BYTES` long.
pub fn layout_text(out: &mut [u8; TEXT_BYTES]) {
    let mut words = Words(42);
    let mut at = 0;
    while at < TEXT_BYTES {
        for &b in words.next_word().as_bytes().iter().chain(b" ") {
            if at < TEXT_BYTES {
                out[at] = b;
                at += 1;
            }
        }
    }
}

/// Greedy word wrap of `text` onto `width` columns: each row's (start, len).
pub fn wrap(text: &[u8], width: usize, mut row: impl FnMut(usize, usize)) {
    let mut start = 0;
    while start < text.len() {
        while start < text.len() && text[start] == b' ' {
            start += 1;
        }
        if start >= text.len() {
            break;
        }
        let limit = (start + width).min(text.len());
        let mut end = limit;
        if limit < text.len()
            && text[limit] != b' '
            && let Some(space) = text[start..limit].iter().rposition(|&b| b == b' ')
        {
            end = start + space;
        }
        row(start, end - start);
        start = end;
    }
}

#[cfg(target_family = "wasm")]
mod guest {
    use super::*;
    use core::ptr::addr_of_mut;

    const ARENA: usize = 1 << 20;
    static mut HEAP: [u8; ARENA] = [0; ARENA];
    static mut TOP: usize = 0;

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        core::arch::wasm32::unreachable()
    }

    fn base() -> *mut u8 {
        // Taking the address reads nothing.
        addr_of_mut!(HEAP) as *mut u8
    }

    /// Bump-allocates `size` bytes at `align`; traps when the arena is full.
    fn alloc(size: usize, align: usize) -> *mut u8 {
        // SAFETY: single-threaded; TOP only moves forward within ARENA.
        unsafe {
            let top = (TOP + align - 1) & !(align - 1);
            if top + size > ARENA {
                core::arch::wasm32::unreachable();
            }
            TOP = top + size;
            base().add(top)
        }
    }

    fn reset() {
        // SAFETY: single-threaded.
        unsafe { TOP = 0 }
    }

    fn put_u32(at: *mut u8, offset: usize, value: u32) {
        // SAFETY: `at + offset` lies in an allocation of the record's size.
        unsafe { (at.add(offset) as *mut u32).write_unaligned(value) }
    }

    /// Copies `bytes` into the arena; returns (ptr, len) as the ABI's string.
    fn string(bytes: &[u8]) -> (u32, u32) {
        let at = alloc(bytes.len().max(1), 1);
        // SAFETY: `at` has room for `bytes`.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len()) };
        (at as u32, bytes.len() as u32)
    }

    /// The `list<T>` result: a pointer to (ptr, len).
    fn list_result(ptr: *mut u8, len: usize) -> i32 {
        let ret = alloc(8, 4);
        put_u32(ret, 0, ptr as u32);
        put_u32(ret, 4, len as u32);
        ret as i32
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cabi_realloc(_old: i32, _old_size: i32, align: i32, size: i32) -> i32 {
        alloc(size as usize, align.max(1) as usize) as i32
    }

    /// `node`: id 0, parent 4, kind 8, rect 12..28, text 28..36; size 36, align 4.
    const NODE: usize = 36;

    #[unsafe(no_mangle)]
    pub extern "C" fn rebuild(event: i32) -> i32 {
        reset();
        let nodes = alloc(NODE * NODES, 4);
        let first = event as u32 as usize % NOTES;
        let mut text = [0u8; 128];
        for i in 0..NODES {
            let (kind, label_len) = match i {
                0 => (0u8, 0),
                1 => {
                    let label = b"Notes: 1000";
                    text[..label.len()].copy_from_slice(label);
                    (1, label.len())
                }
                n if n == NODES - 1 => {
                    let label = b"ok 1000 notes";
                    text[..label.len()].copy_from_slice(label);
                    (3, label.len())
                }
                n => {
                    text[0] = b'-';
                    text[1] = b' ';
                    (2, 2 + note_text((first + n) % NOTES, &mut text[2..]))
                }
            };
            let (ptr, len) = string(&text[..label_len]);
            // SAFETY: node `i` lies inside the `nodes` allocation.
            let node = unsafe { nodes.add(i * NODE) };
            put_u32(node, 0, i as u32);
            put_u32(node, 4, 0);
            // SAFETY: as above; the kind is one byte.
            unsafe { node.add(8).write(kind) };
            put_u32(node, 12, 0);
            put_u32(node, 16, i as u32);
            put_u32(node, 20, 240);
            put_u32(node, 24, 1);
            put_u32(node, 28, ptr);
            put_u32(node, 32, len);
        }
        list_result(nodes, NODES)
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cabi_post_rebuild(_ret: i32) {
        reset();
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn layout(width: i32, height: i32) -> i32 {
        reset();
        let (width, height) = (width.max(1) as usize, height.max(1) as usize);
        let mut text = [0u8; TEXT_BYTES];
        layout_text(&mut text);
        // The cell grid, one byte a cell, then one string per row.
        let cells = alloc(width * height, 1);
        // SAFETY: `cells` holds width * height bytes.
        unsafe { core::ptr::write_bytes(cells, b' ', width * height) };
        let mut row = 0;
        wrap(&text, width, |start, len| {
            if row < height {
                // SAFETY: `len` <= width, so the row fits in its cells.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        text.as_ptr().add(start),
                        cells.add(row * width),
                        len,
                    )
                };
                row += 1;
            }
        });
        let rows = alloc(8 * height, 4);
        for r in 0..height {
            // SAFETY: row `r`'s cells lie inside `cells`.
            let line = unsafe { core::slice::from_raw_parts(cells.add(r * width), width) };
            let used = line.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
            let (ptr, len) = string(&line[..used]);
            // SAFETY: entry `r` lies inside `rows`.
            let entry = unsafe { rows.add(8 * r) };
            put_u32(entry, 0, ptr);
            put_u32(entry, 4, len);
        }
        list_result(rows, height)
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cabi_post_layout(_ret: i32) {
        reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_keeps_rows_within_width_and_loses_no_word() {
        let mut text = [0u8; TEXT_BYTES];
        layout_text(&mut text);
        let mut rows = Vec::new();
        wrap(&text, 240, |start, len| rows.push((start, len)));
        assert!(rows.iter().all(|&(_, len)| len <= 240));
        let words: usize = rows
            .iter()
            .map(|&(s, l)| {
                text[s..s + l]
                    .split(|&b| b == b' ')
                    .filter(|w| !w.is_empty())
                    .count()
            })
            .sum();
        let all = text.split(|&b| b == b' ').filter(|w| !w.is_empty()).count();
        assert_eq!(words, all);
        assert!(rows.len() > 30 && rows.len() <= 67, "{} rows", rows.len());
    }

    #[test]
    fn notes_are_words() {
        let mut out = [0u8; 128];
        let len = note_text(7, &mut out);
        assert!(
            len > 3
                && out[..len]
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || *b == b' ')
        );
    }
}
