use std::collections::BTreeMap;

use super::*;
use crate::release_check::{Job, Run};

/// A fixture commit `abc`: its files, `release/seq`, and Release run logs.
#[derive(Default)]
struct Fixture {
    files: BTreeMap<&'static str, Vec<u8>>,
    seq: Option<&'static str>,
    release_runs: Vec<Run>,
    logs: BTreeMap<u64, String>,
}

impl Files for Fixture {
    fn file(&self, _: &str, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }
}

impl Facts for Fixture {
    fn sha(&self, rev: &str) -> Result<String, String> {
        Ok(rev.to_string())
    }
    fn seq(&self, rev: &str) -> Option<String> {
        // The last tag's seq is 1; the commit's is the fixture's.
        match rev == "v0.1.0-rc.1" {
            true => Some("1\n".into()),
            false => self.seq.map(|s| format!("{s}\n")),
        }
    }
    fn last_tag(&self) -> Result<Option<String>, String> {
        Ok(Some("v0.1.0-rc.1".into()))
    }
    fn same_model_input(&self, _: &str, _: &str) -> Option<bool> {
        Some(true)
    }
    fn runs(&self, workflow: &str, _: Which) -> Result<Vec<Run>, String> {
        Ok(match workflow {
            "release.yml" => self.release_runs.clone(),
            _ => Vec::new(),
        })
    }
    fn jobs(&self, _: u64) -> Result<Vec<Job>, String> {
        Ok(Vec::new())
    }
    fn job_log(&self, _: u64) -> Result<String, String> {
        Err("no job log".into())
    }
    fn log(&self, id: u64) -> Result<String, String> {
        self.logs.get(&id).cloned().ok_or(format!("no log {id}"))
    }
}

fn run(id: u64, event: &str, conclusion: &str) -> Run {
    Run {
        id,
        sha: "abc".into(),
        event: event.into(),
        conclusion: conclusion.into(),
    }
}

const MODELS: &str = r#"{"default":"m1","models":[
  {"id":"m0","repo":"o/zero","file":"zero.gguf","revision":"r0","sha256":"00","size":1,"license":"MIT"},
  {"id":"m1","repo":"o/one","file":"one.gguf","revision":"r1","sha256":"ab12","size":42,"license":"Apache-2.0"}]}"#;

fn ready() -> Fixture {
    let mut f = Fixture {
        seq: Some("2"),
        release_runs: vec![run(77, "push", "success")],
        ..Fixture::default()
    };
    f.files.insert("xtask/models.json", MODELS.into());
    f.files.insert("NOTICES.txt", b"abc".to_vec());
    f.logs.insert(
        77,
        "build-a\tBuild\t2026-11-27T00:00:00Z xtask: size 1048576 bytes (1 MiB)\n\
         build-b\tBuild\t2026-11-27T00:00:00Z xtask: size 1048576 bytes (1 MiB)\n"
            .into(),
    );
    f
}

#[test]
fn the_draft_has_each_pin_the_notices_hash_the_size_and_the_checklist() {
    let text = draft(&ready(), &ready(), "abc").unwrap();
    for want in [
        "# Release notes draft (abc)",
        "- release/seq: `2`",
        "## Model",
        // The default model's pin, not the first model's.
        "- `m1`: `o/one`, file `one.gguf`, revision `r1`",
        "- SHA-256 `ab12`, 42 bytes, licence Apache-2.0",
        "## Files",
        // sha256("abc")
        "- NOTICES.txt: SHA-256 `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`, 3 bytes",
        "- image: 1048576 bytes (1 MiB), Release run 77 (push), 2 builds",
        "## Checklist",
        "- met: release/seq above the last release's: 2, above v0.1.0-rc.1's 1",
        "- met: NOTICES.txt the same in the image and the release: Release run 77 (push) at abc passed",
        "- founder's: the release signed by the founder's root",
    ] {
        assert!(text.contains(want), "{want:?} not in:\n{text}");
    }
    assert!(!text.contains("m0"), "{text}");
}

#[test]
fn a_part_it_cannot_find_is_missing_never_left_out() {
    let mut f = ready();
    f.files.remove("NOTICES.txt");
    f.files.insert(
        "xtask/models.json",
        br#"{"default":"gone","models":[]}"#.to_vec(),
    );
    f.seq = None;
    f.release_runs.clear();
    let text = draft(&f, &f, "abc").unwrap();
    for want in [
        "- release/seq: MISSING: no release/seq",
        "- MISSING: the default model `gone` is not in the catalog",
        "- NOTICES.txt: MISSING: no NOTICES.txt",
        "- image: MISSING: no green Release run at abc",
    ] {
        assert!(text.contains(want), "{want:?} not in:\n{text}");
    }
    let mut g = ready();
    g.files.insert("xtask/models.json", b"not json".to_vec());
    assert!(
        draft(&g, &g, "abc")
            .unwrap()
            .contains("- MISSING: xtask/models.json is not JSON")
    );
}

#[test]
fn image_sizes_that_differ_are_listed_and_a_log_with_none_is_missing() {
    let log = "x xtask: size 10 bytes (0 MiB)\nx xtask: size 12 bytes (0 MiB)\nx xtask: size 10 bytes (0 MiB)\nxtask: size many bytes\n";
    assert_eq!(sizes(log), vec![(10, 2), (12, 1)]);
    let mut f = ready();
    f.logs.insert(77, log.into());
    assert!(
        draft(&f, &f, "abc")
            .unwrap()
            .contains("- image: builds DIFFER, Release run 77 (push): 10 bytes x2, 12 bytes x1")
    );
    f.logs.insert(77, "nothing here\n".into());
    assert!(
        draft(&f, &f, "abc")
            .unwrap()
            .contains("MISSING: Release run 77 (push) printed no image size")
    );
}

#[test]
fn a_push_run_is_taken_over_a_pull_request_run_at_one_sha() {
    let mut f = ready();
    let log = |bytes: u64| format!("x xtask: size {bytes} bytes (0 MiB)\n");
    // The pull_request run is newer and listed first; the push run is older.
    f.release_runs = vec![
        run(90, "pull_request", "success"),
        run(80, "push", "success"),
    ];
    f.logs.insert(90, log(111));
    f.logs.insert(80, log(222));
    let text = draft(&f, &f, "abc").unwrap();
    assert!(
        text.contains("- image: 222 bytes (0 MiB), Release run 80 (push), 1 builds"),
        "{text}"
    );
    // Alone, a pull_request run is used, and named for what it built.
    f.release_runs = vec![run(90, "pull_request", "success")];
    let text = draft(&f, &f, "abc").unwrap();
    assert!(
        text.contains("Release run 90 (pull_request: REV merged into its base)"),
        "{text}"
    );
}
