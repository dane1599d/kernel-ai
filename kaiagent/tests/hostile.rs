//! An agent manifest read as hostile (ADR 0013 section 6): the `agent_manifest` fuzz
//! target's body (`fuzz/agent_harness.rs`), run here over truncations and mutations so a
//! harness that cannot build, or that panics on its own, fails the host tests without
//! libFuzzer. Cut, mutated or bent at every byte, a manifest is read or refused with one
//! sentence, never a panic; what is read allocates no more than its bytes could hold, is
//! written back as a manifest that reads back as the same one, and grants no row, scope,
//! host or budget its text does not name.
//!
//! The text is a package's own, so every case here stands for a package an attacker
//! made: the parser's strictness (every line, once, in order) is what these hit.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/agent_harness.rs"]
mod harness;

use harness::SAMPLES;
use kaiagent::{Manifest, Refusal};

/// The parse is a few small strings and vectors; the rest grows with the input.
const ALLOC_PER_BYTE: usize = 32;
const ALLOC_FLOOR: usize = 64 * 1024;

/// Manifests that parse: the harness's, and one whose hosts are at netdest's limits.
fn samples() -> Vec<String> {
    let host = |c: char| {
        let label = c.to_string().repeat(60);
        format!("{label}.{label}.{label}.{label}.example")
    };
    let limits = format!(
        "kai-app 1\nid maxed\nabi 1.0\nagent 1\nclass background\n\
         model tokens/day=2000000 kv=2048 pool=512m\nactions fs_list fs_read fs_create\n\
         memory read write\nnet ports:443,hosts:{}+{}+{} bytes/day=512m\n",
        host('a'),
        host('b'),
        host('c')
    );
    SAMPLES
        .iter()
        .map(|s| s.to_string())
        .chain([limits])
        .collect()
}

/// What one mutant is held to: the fuzz target's body, and a bound on what one parse
/// allocates.
fn read(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let (_, peak) = hostile::peak_alloc(|| Manifest::parse(&text));
    assert!(
        peak <= ALLOC_FLOOR + ALLOC_PER_BYTE * bytes.len(),
        "{peak} bytes for {} bytes",
        bytes.len()
    );
    harness::manifest(bytes);
    harness::manifest(text.as_bytes());
}

#[test]
fn the_samples_read_and_round_trip() {
    for sample in samples() {
        let manifest = harness::read(&sample, &agentabi::ROWS).unwrap();
        assert_eq!(Manifest::parse(&manifest.format()).unwrap(), manifest);
        harness::manifest(sample.as_bytes());
    }
}

#[test]
fn mutated_manifests_are_read_or_refused_and_what_is_read_reads_back() {
    for (n, sample) in samples().iter().enumerate() {
        hostile::fuzz(sample.as_bytes(), 41 + n as u64, 40_000, read);
    }
}

#[test]
fn every_prefix_of_a_manifest_is_read_or_refused() {
    for sample in samples() {
        hostile::truncations(sample.as_bytes(), |_, cut| read(cut));
    }
}

#[test]
fn every_line_dropped_doubled_or_swapped_is_refused() {
    for sample in samples() {
        let lines: Vec<&str> = sample.split_inclusive('\n').collect();
        for at in 0..lines.len() {
            let mut dropped: Vec<&str> = lines.clone();
            dropped.remove(at);
            let mut doubled: Vec<&str> = lines.clone();
            doubled.insert(at, lines[at]);
            for edit in [dropped, doubled] {
                assert!(Manifest::parse(&edit.concat()).is_err(), "{edit:?}");
            }
            if at + 1 < lines.len() {
                let mut swapped = lines.clone();
                swapped.swap(at, at + 1);
                assert!(Manifest::parse(&swapped.concat()).is_err(), "{swapped:?}");
            }
        }
    }
}

/// Every shared suffix, as an attacker's package would write it (alone, among its
/// own hosts in any place, in any case, exact or not), is refused by name, and the
/// same manifest naming a host under it is read.
#[test]
fn every_shared_suffix_is_refused_by_name_wherever_and_however_it_is_written() {
    let manifest = |net: &str| {
        format!(
            "kai-app 1\nid weather\nabi 1.0\nagent 1\nclass interactive\nmodel -\n\
             actions fs_list\nmemory read\nnet {net}\n"
        )
    };
    for shared in netdest::SHARED_SUFFIXES {
        let upper = shared.to_ascii_uppercase();
        let mixed: String = shared
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 0 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        for written in [shared.to_string(), upper, mixed] {
            for hosts in [
                written.clone(),
                format!("{written}+a.example"),
                format!("a.example+b.example+{written}"),
            ] {
                for exact in ["", "match:exact,"] {
                    let net = format!("proto:tcp,ports:443,{exact}hosts:{hosts}");
                    let got = Manifest::parse(&manifest(&net));
                    assert_eq!(got, Err(Refusal::SharedHost(shared.to_string())), "{net}");
                }
            }
        }
        let under = format!("proto:tcp,ports:443,hosts:own.{shared}");
        assert!(Manifest::parse(&manifest(&under)).is_ok(), "{under}");
    }
}
