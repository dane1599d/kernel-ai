//! Cases booted only when named (`XTASK_CASE`), never in `cargo xtask test`'s
//! shards: `desk-tour` takes a screenshot of each step of the desktop tour
//! (`kernel/src/desk_tour_tests.rs`) for `.github/workflows/screenshots.yml`.

use super::{TestCase, case};

/// One stage a step: its screendump.
const KEYS: &[&[&str]] = &[
    &["!screendump tour-01-desktop.ppm"],
    &["!screendump tour-02-apps.ppm"],
    &["!screendump tour-03-files.ppm"],
    &["!screendump tour-04-terminal.ppm"],
    &["!screendump tour-05-notes.ppm"],
    &["!screendump tour-06-settings-appearance.ppm"],
    &["!screendump tour-07-settings-access.ppm"],
    &["!screendump tour-08-monitor.ppm"],
    &["!screendump tour-09-kai-asks.ppm"],
    &["!screendump tour-10-kai-done.ppm"],
    &["!screendump tour-11-light.ppm"],
    &["!screendump tour-12-overlap.ppm"],
];

pub const CASES: &[TestCase] = &[TestCase {
    keys: KEYS,
    expect: &["desk-tour: the tour is done"],
    // Files and the saved theme live on `kai-data`.
    data: Some(crate::blklog::Data {
        nvme: false,
        log: None,
        gpt: true,
    }),
    // Five apps of 8192 pages each beside the desktop and the shell.
    memory_mib: Some(2048),
    ..case("desk-tour", "test=desk-tour")
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's steps, in the order it logs "ready for keys".
    const STEPS: &[&str] = &[
        "01-desktop",
        "02-apps",
        "03-files",
        "04-terminal",
        "05-notes",
        "06-settings-appearance",
        "07-settings-access",
        "08-monitor",
        "09-kai-asks",
        "10-kai-done",
        "11-light",
        "12-overlap",
    ];

    #[test]
    fn each_step_has_one_screendump_named_for_it() {
        assert_eq!(KEYS.len(), STEPS.len());
        for (stage, step) in KEYS.iter().zip(STEPS) {
            assert_eq!(*stage, [format!("!screendump tour-{step}.ppm").as_str()]);
        }
        // The kernel takes its pictures in the same order, each once.
        let kernel = include_str!("../../../kernel/src/desk_tour_tests.rs");
        let shots: Vec<&str> = kernel
            .split("shot(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|name| !name.is_empty() && !name.contains(' '))
            .collect();
        assert_eq!(shots, STEPS);
    }

    #[test]
    fn the_tour_is_not_in_the_shards() {
        assert!(
            super::super::test_cases()
                .iter()
                .all(|c| c.name != "desk-tour")
        );
    }
}
