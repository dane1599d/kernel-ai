//! Byte-pair merges learned from a text, for generated models whose prompts must
//! be short: trained on the system prompts a boot case sends, they make each a few
//! hundred tokens instead of one per byte, so the case runs in seconds even when
//! QEMU emulates the CPU.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Merges learned from `text` as BPE training learns them: the text is split as
/// the qwen2 pretokenizer splits it, then the most frequent pair of neighbouring
/// symbols, the first in the text among equals, is joined everywhere, again and
/// again, until `max` merges are learned or no piece has two symbols left.
/// Returns the merges, `"a b"` in the byte alphabet, and the new tokens they
/// make, both in the order learned.
pub(super) fn train(text: &str, max: usize) -> (Vec<String>, Vec<String>) {
    // Symbols are ids into `names`; the first 256 are the bytes.
    let mut names: Vec<String> = (0..=255u8).map(|b| bpe::bytes::to_char(b).into()).collect();
    let mut pieces: Vec<Vec<u32>> = bpe::pretok::split(text)
        .map(|piece| piece.bytes().map(u32::from).collect())
        .collect();
    let (mut merges, mut made) = (Vec::new(), Vec::new());
    while merges.len() < max {
        // Each pair's count and where it first comes.
        let mut pairs: BTreeMap<(u32, u32), (usize, usize)> = BTreeMap::new();
        let mut at = 0;
        for piece in &pieces {
            for pair in piece.windows(2) {
                pairs.entry((pair[0], pair[1])).or_insert((0, at)).0 += 1;
                at += 1;
            }
        }
        let best = pairs
            .iter()
            .max_by(|(_, x), (_, y)| x.0.cmp(&y.0).then(y.1.cmp(&x.1)))
            .map(|(&pair, _)| pair);
        let Some((a, b)) = best else {
            break;
        };
        let (left, right) = (names[a as usize].clone(), names[b as usize].clone());
        merges.push(format!("{left} {right}"));
        let joined = left + &right;
        let id = match names.iter().position(|name| *name == joined) {
            Some(id) => id as u32,
            None => {
                made.push(joined.clone());
                names.push(joined);
                (names.len() - 1) as u32
            }
        };
        for piece in &mut pieces {
            let mut i = 0;
            while i + 1 < piece.len() {
                if (piece[i], piece[i + 1]) == (a, b) {
                    piece[i] = id;
                    piece.remove(i + 1);
                }
                i += 1;
            }
        }
    }
    (merges, made)
}
