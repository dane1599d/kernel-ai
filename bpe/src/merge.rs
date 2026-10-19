//! Byte-pair merging of one piece: start from one token per byte, then merge the
//! adjacent pair with the lowest rank, the leftmost first among equal ranks, until
//! no pair has a merge. A heap of candidate pairs over a linked list keeps a long
//! piece (a line of `=` signs, say) at n log n rather than n squared.

use crate::Tokenizer;
use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Reverse;

/// The end of the list, and the id of a symbol merged into its left neighbour.
const NONE: u32 = u32::MAX;
/// Pieces are cut into runs this long, so positions fit in a u32 with room for
/// NONE; a single piece this long is not text anyone types.
const MAX_RUN: usize = 1 << 30;

/// One symbol of the piece, linked to its neighbours by position.
struct Symbol {
    id: u32,
    prev: u32,
    next: u32,
}

/// A pair that could merge: its rank, the left symbol's position, and the two
/// ids it was seen with, so a pair that has since changed is skipped.
type Candidate = Reverse<(u32, u32, u32, u32)>;

pub(crate) fn merge_piece(tok: &Tokenizer, bytes: &[u8], out: &mut Vec<u32>) {
    for run in bytes.chunks(MAX_RUN) {
        merge_run(tok, run, out);
    }
}

fn merge_run(tok: &Tokenizer, bytes: &[u8], out: &mut Vec<u32>) {
    if bytes.len() < 2 {
        out.extend(bytes.iter().map(|&b| tok.byte_token[b as usize]));
        return;
    }
    let last = bytes.len() - 1;
    let mut symbols: Vec<Symbol> = bytes
        .iter()
        .enumerate()
        .map(|(i, &b)| Symbol {
            id: tok.byte_token[b as usize],
            prev: if i == 0 { NONE } else { i as u32 - 1 },
            next: if i == last { NONE } else { i as u32 + 1 },
        })
        .collect();
    let mut heap: BinaryHeap<Candidate> = BinaryHeap::with_capacity(bytes.len());
    let push = |heap: &mut BinaryHeap<Candidate>, symbols: &[Symbol], left: u32| {
        let right = symbols[left as usize].next;
        if right == NONE {
            return;
        }
        let (l, r) = (symbols[left as usize].id, symbols[right as usize].id);
        if let Some((rank, _)) = tok.merge_of(l, r) {
            heap.push(Reverse((rank, left, l, r)));
        }
    };
    for i in 0..last as u32 {
        push(&mut heap, &symbols, i);
    }
    while let Some(Reverse((_, left, l, r))) = heap.pop() {
        let right = symbols[left as usize].next;
        // A merged symbol only grows, so ids that still match mean the pair is
        // still there.
        if symbols[left as usize].id != l || right == NONE || symbols[right as usize].id != r {
            continue;
        }
        let Some((_, merged)) = tok.merge_of(l, r) else {
            continue;
        };
        symbols[left as usize].id = merged;
        let after = symbols[right as usize].next;
        symbols[left as usize].next = after;
        if after != NONE {
            symbols[after as usize].prev = left;
        }
        // Mark the right symbol dead, so candidates still queued at it fail the
        // id check (no token id is NONE).
        symbols[right as usize].id = NONE;
        let before = symbols[left as usize].prev;
        if before != NONE {
            push(&mut heap, &symbols, before);
        }
        push(&mut heap, &symbols, left);
    }
    let mut at = 0;
    while at != NONE {
        out.push(symbols[at as usize].id);
        at = symbols[at as usize].next;
    }
}
