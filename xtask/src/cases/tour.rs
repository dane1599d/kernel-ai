//! Cases booted only when named (`XTASK_CASE`), never in `cargo xtask test`'s
//! shards: `desk-tour`, `desk-tour-1024` and `desk-tour-1080` take a
//! screenshot of each step of the desktop tour
//! (`kernel/src/desk_tour_tests.rs`) at 1280x800, 1024x768 and 1920x1080,
//! for `.github/workflows/screenshots.yml`.

use super::{TestCase, case};

/// One stage a step: its screendump, `tour-<width>-<step>.ppm`.
macro_rules! keys {
    ($w:literal) => {
        &[
            &[concat!("!screendump tour-", $w, "-01-desktop.ppm")],
            &[concat!("!screendump tour-", $w, "-02-start.ppm")],
            &[concat!("!screendump tour-", $w, "-03-search.ppm")],
            &[concat!("!screendump tour-", $w, "-04-all-apps.ppm")],
            &[concat!("!screendump tour-", $w, "-05-files.ppm")],
            &[concat!("!screendump tour-", $w, "-06-terminal.ppm")],
            &[concat!("!screendump tour-", $w, "-07-notes.ppm")],
            &[concat!(
                "!screendump tour-",
                $w,
                "-08-settings-appearance.ppm"
            )],
            &[concat!("!screendump tour-", $w, "-09-settings-access.ppm")],
            &[concat!("!screendump tour-", $w, "-10-monitor.ppm")],
            &[concat!("!screendump tour-", $w, "-11-kai-pane.ppm")],
            &[concat!("!screendump tour-", $w, "-12-kai-asks.ppm")],
            &[concat!("!screendump tour-", $w, "-13-kai-done.ppm")],
            &[concat!("!screendump tour-", $w, "-14-light.ppm")],
            &[concat!("!screendump tour-", $w, "-15-overlap.ppm")],
        ]
    };
}

/// The tour at `size` on virtio-gpu, which takes `display=`.
macro_rules! tour {
    ($name:literal, $w:literal, $size:literal) => {
        TestCase {
            gpu: true,
            keys: keys!($w),
            expect: &[
                concat!("desktop: drew the desk at ", $size),
                "desk-tour: the tour is done",
            ],
            // Files and the saved theme live on `kai-data`.
            data: Some(crate::blklog::Data {
                nvme: false,
                log: None,
                gpt: true,
            }),
            // Five apps of 8192 pages each beside the desktop and the shell.
            memory_mib: Some(2048),
            ..case($name, concat!("test=desk-tour display=", $size))
        }
    };
}

pub const CASES: &[TestCase] = &[
    tour!("desk-tour", "1280", "1280x800"),
    tour!("desk-tour-1024", "1024", "1024x768"),
    tour!("desk-tour-1080", "1920", "1920x1080"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's steps, in the order it logs "ready for keys".
    const STEPS: &[&str] = &[
        "01-desktop",
        "02-start",
        "03-search",
        "04-all-apps",
        "05-files",
        "06-terminal",
        "07-notes",
        "08-settings-appearance",
        "09-settings-access",
        "10-monitor",
        "11-kai-pane",
        "12-kai-asks",
        "13-kai-done",
        "14-light",
        "15-overlap",
    ];

    #[test]
    fn each_step_has_one_screendump_named_for_it_at_each_size() {
        for (case, width) in CASES.iter().zip(["1280", "1024", "1920"]) {
            assert_eq!(case.keys.len(), STEPS.len(), "{}", case.name);
            for (stage, step) in case.keys.iter().zip(STEPS) {
                let want = format!("!screendump tour-{width}-{step}.ppm");
                assert_eq!(*stage, [want.as_str()], "{}", case.name);
            }
            assert!(case.cmdline.contains(&format!("display={width}x")));
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
                .all(|c| !c.name.starts_with("desk-tour"))
        );
    }
}
