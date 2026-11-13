//! An agent manifest read as hostile (ADR 0013 section 6): cut, mutated or bent at
//! every byte, it is read or refused, never a panic; what is read allocates no more
//! than its bytes could hold; and what is read is written back as a manifest that reads
//! back as the same one, so a package's manifest can be shown, stored and read again.
//!
//! The text is a package's own, so every case here stands for a package an attacker
//! made: the parser's strictness (every line, once, in order) is what these hit.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kaiagent::Manifest;

/// Manifests that parse: the ADR's example, an interactive one with the net line (and
/// its budget), and one at the limits.
fn samples() -> Vec<String> {
    let background = "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass background\n\
        model tokens/day=50000 kv=1024 pool=64m\nactions fs_list fs_read notes_*\n\
        memory read write\nnet none\n";
    let net = "kai-app 1\nid weather\nabi 1.0\nagent 1\nclass interactive\n\
        model -\nactions fs_list\nmemory read\n\
        net proto:tcp,ports:443,hosts:api.weather.example bytes/day=64m\n";
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
    vec![background.into(), net.into(), limits]
}

/// What one mutant is held to.
fn read(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let (parsed, peak) = hostile::peak_alloc(|| Manifest::parse(&text));
    assert!(
        peak <= 64 * 1024 + 32 * bytes.len(),
        "{peak} bytes for {} bytes",
        bytes.len()
    );
    if let Ok(manifest) = parsed {
        // What was read is written as a manifest that reads back as the same one.
        let written = manifest.format();
        assert_eq!(
            Manifest::parse(&written).as_ref(),
            Ok(&manifest),
            "{text:?} was read as {manifest:?}, which is written as {written:?}"
        );
        // And the written form is the one form: it writes itself.
        assert_eq!(Manifest::parse(&written).unwrap().format(), written);
    }
}

#[test]
fn the_samples_read_and_round_trip() {
    for sample in samples() {
        let manifest = Manifest::parse(&sample).unwrap();
        assert_eq!(Manifest::parse(&manifest.format()).unwrap(), manifest);
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
