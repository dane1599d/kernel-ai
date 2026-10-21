//! `llmreplay` and the tune trace (M10a).
//!
//! A replay file stands where a model would, on the `kai-model` disk: the llm
//! service answers a `tune` request from it only when the request's prompt hash
//! is recorded there, and refuses anything else. So a whole boot, the shell's
//! flow included, runs on a recorded real-model answer with no network and no
//! model, which is what an icount boot needs: nothing host-timed, and the
//! answer exactly the one recorded. File: [`kabi::model::REPLAY_MAGIC`], then
//! one line per answer, `<prompt hash, 16 hex digits> <completion>`, the
//! completion as the llm wire sends it ([`super::encode_completion`]) with `\`,
//! newlines and carriage returns escaped.
//!
//! The trace: for every `tune` request the llm service prints the request it
//! got and the completion it sent back, as hex in numbered pieces short enough
//! for a console line, so xtask can rebuild them from a boot's transcript and
//! recompute the host engine's answer to the very same request.

use super::{TuneRequest, decode_completion, encode_completion};
use crate::chat::Completion;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The replay file's first line.
pub const MAGIC: &str = "KAIREPLAY1\n";
/// Hex characters per trace line.
pub const TRACE_PIECE: usize = 120;

/// FNV-1a over `bytes`, as `kabi::model::fnv1a`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The hash a replay is keyed by: the conversation the model would see, every
/// message's role and text, so any change to the summary, the names it may
/// label or the cards changes it.
pub fn prompt_hash(request: &TuneRequest) -> u64 {
    let chat = request.chat();
    let mut text = String::new();
    for message in &chat.messages {
        text.push_str(&format!("{:?}\n{}\n", message.role, message.content));
    }
    text.push_str(&request.labelable.join(","));
    fnv1a(text.as_bytes())
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            ch => out.push(ch),
        }
    }
    out
}

fn unescape(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next()? {
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                _ => return None,
            },
            ch => out.push(ch),
        }
    }
    Some(out)
}

/// A replay file holding `entries`.
pub fn encode(entries: &[(u64, Completion)]) -> String {
    let mut file = String::from(MAGIC);
    for (hash, completion) in entries {
        file.push_str(&format!(
            "{hash:016x} {}\n",
            escape(&encode_completion(completion))
        ));
    }
    file
}

/// Why a replay cannot answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    NotReplay,
    Malformed(usize),
    NotRecorded(u64),
}

impl ReplayError {
    pub fn describe(&self) -> String {
        match self {
            ReplayError::NotReplay => String::from("llmreplay: the disk holds no replay file"),
            ReplayError::Malformed(line) => format!("llmreplay: line {line} is malformed"),
            ReplayError::NotRecorded(hash) => {
                format!("llmreplay: no recorded answer for prompt {hash:016x}")
            }
        }
    }
}

/// The recorded answer to `request`, strictly: the same prompt hash or nothing.
pub fn answer(file: &[u8], request: &TuneRequest) -> Result<Completion, ReplayError> {
    let text = core::str::from_utf8(file).map_err(|_| ReplayError::NotReplay)?;
    let body = text.strip_prefix(MAGIC).ok_or(ReplayError::NotReplay)?;
    let hash = prompt_hash(request);
    for (i, line) in body.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
        let malformed = || ReplayError::Malformed(i + 2);
        let (key, wire) = line.split_once(' ').ok_or_else(malformed)?;
        let key = u64::from_str_radix(key, 16).map_err(|_| malformed())?;
        if key == hash {
            let wire = unescape(wire).ok_or_else(malformed)?;
            return decode_completion(&wire).ok_or_else(malformed);
        }
    }
    Err(ReplayError::NotRecorded(hash))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The console lines that trace message `seq`'s `kind` (`request` or `answer`).
pub fn trace(kind: &str, seq: u64, text: &str) -> Vec<String> {
    let hex = hex(text.as_bytes());
    let pieces: Vec<&str> = hex
        .as_bytes()
        .chunks(TRACE_PIECE)
        .map(|c| core::str::from_utf8(c).unwrap_or(""))
        .collect();
    let n = pieces.len().max(1);
    let pieces = if pieces.is_empty() {
        alloc::vec![""]
    } else {
        pieces
    };
    pieces
        .iter()
        .enumerate()
        .map(|(i, piece)| format!("tune-trace {kind} {seq} {}/{n} {piece}", i + 1))
        .collect()
}

/// A message being rebuilt: its seq, its piece count and the pieces seen, numbered.
type Traced = (u64, usize, Vec<(usize, String)>);

/// Every complete traced message of `kind` in a transcript, by `seq`, in order;
/// any prefix before `tune-trace` on a line is ignored. A later piece with the
/// same number replaces an earlier one. Pieces are kept as they come, so memory
/// follows the transcript's size whatever piece counts its lines claim.
pub fn untrace<'a>(lines: impl Iterator<Item = &'a str>, kind: &str) -> Vec<(u64, String)> {
    let mut found: Vec<Traced> = Vec::new();
    let mut by_seq: BTreeMap<u64, usize> = BTreeMap::new();
    for line in lines {
        let Some(at) = line.find("tune-trace ") else {
            continue;
        };
        let mut words = line[at..].split(' ').skip(1);
        let (Some(k), Some(seq), Some(part), piece) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            continue;
        };
        let (Ok(seq), Some((i, n))) = (seq.parse::<u64>(), part.split_once('/')) else {
            continue;
        };
        let (Ok(i), Ok(n)) = (i.parse::<usize>(), n.parse::<usize>()) else {
            continue;
        };
        if k != kind || i == 0 || i > n || n > 10_000 {
            continue;
        }
        let at = *by_seq.entry(seq).or_insert_with(|| {
            found.push((seq, n, Vec::new()));
            found.len() - 1
        });
        let (_, count, pieces) = &mut found[at];
        // The count the message's first line gave holds.
        if i <= *count {
            pieces.push((i, String::from(piece.unwrap_or("").trim_end())));
        }
    }
    found
        .into_iter()
        .filter_map(|(seq, count, mut pieces)| {
            // Latest first, so that the stable sort and dedup keep the latest.
            pieces.reverse();
            pieces.sort_by_key(|&(i, _)| i);
            pieces.dedup_by_key(|&mut (i, _)| i);
            // Numbers 1 to count, each once: complete.
            if pieces.len() != count {
                return None;
            }
            let hex: String = pieces.into_iter().map(|(_, piece)| piece).collect();
            let bytes: Option<Vec<u8>> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
                .collect();
            Some((seq, String::from_utf8(bytes?).ok()?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str) -> TuneRequest {
        TuneRequest {
            labelable: alloc::vec![String::from("build-1"), String::from("indexer")],
            text: String::from(text),
        }
    }

    #[test]
    fn a_replay_answers_only_the_prompt_it_recorded() {
        let recorded = Completion {
            text: String::from("{\"reason\":\"a \\\\ b\nc\",\"action\":\"keep\"}"),
            cut_off: true,
        };
        let asked = request("Keep \"editor\" responsive.");
        let file = encode(&[(prompt_hash(&asked), recorded.clone())]);
        assert!(file.as_bytes().starts_with(kabi_magic()));
        assert_eq!(answer(file.as_bytes(), &asked), Ok(recorded));
        let other = request("Keep \"editor\" responsive!");
        assert_eq!(
            answer(file.as_bytes(), &other),
            Err(ReplayError::NotRecorded(prompt_hash(&other)))
        );
        let mut fewer = asked.clone();
        fewer.labelable.pop();
        assert_ne!(prompt_hash(&fewer), prompt_hash(&asked));
        assert_eq!(answer(b"GGUF", &asked), Err(ReplayError::NotReplay));
        let broken = format!("{MAGIC}zz =x\n");
        assert_eq!(
            answer(broken.as_bytes(), &asked),
            Err(ReplayError::Malformed(2))
        );
    }

    fn kabi_magic() -> &'static [u8] {
        MAGIC.as_bytes()
    }

    #[test]
    fn traces_come_back_whole_from_a_transcript() {
        let long: String = (0..300)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let mut lines: Vec<String> = trace("request", 1, &long)
            .into_iter()
            .map(|l| format!("  | [llm] {l}"))
            .collect();
        lines.extend(trace("request", 2, "é\nx"));
        lines.extend(trace("answer", 1, ""));
        lines.push(String::from("tune-trace request 3 1/2 6162"));
        let back = untrace(lines.iter().map(String::as_str), "request");
        assert_eq!(back, [(1, long), (2, String::from("é\nx"))]);
        assert_eq!(
            untrace(lines.iter().map(String::as_str), "answer"),
            [(1, String::new())]
        );
    }
}
