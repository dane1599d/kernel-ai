//! `kai-slot 1` (ADR 0008 section 2): a slot's release record, `boot/slot` on
//! the slot's volume and `slot` in its bundle. One `key value` per line after the
//! header line; unknown keys are skipped, so later versions can add keys.

/// The header line.
pub const HEADER: &str = "kai-slot 1";

/// The fields the boot path needs. The record also names the version, commit,
/// channel, ABI and formats, which only people and `updated` read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotFile<'a> {
    pub release_seq: u32,
    pub epoch: u32,
    pub version: &'a str,
    pub channel: &'a str,
}

/// Parses a record; `None` without the header, or with a key the boot needs
/// missing or malformed.
pub fn parse(text: &str) -> Option<SlotFile<'_>> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != HEADER {
        return None;
    }
    let (mut release_seq, mut epoch, mut version, mut channel) = (None, None, "", "");
    for line in lines {
        let Some((key, value)) = line.trim_end().split_once(' ') else {
            continue;
        };
        match key {
            "release_seq" => release_seq = Some(value.parse().ok()?),
            "epoch" => epoch = Some(value.parse().ok()?),
            "version" => version = value,
            "channel" => channel = value,
            _ => {}
        }
    }
    Some(SlotFile {
        release_seq: release_seq?,
        epoch: epoch?,
        version,
        channel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_boot_fields_and_skips_the_rest() {
        let text = "kai-slot 1\nrelease_seq 7\nepoch 2\nversion 0.1.0-dev\ncommit abc\nchannel dev\nlater-key x\n";
        let file = parse(text).unwrap();
        assert_eq!(
            file,
            SlotFile {
                release_seq: 7,
                epoch: 2,
                version: "0.1.0-dev",
                channel: "dev"
            }
        );
    }

    #[test]
    fn refuses_a_wrong_header_or_a_missing_or_bad_number() {
        assert_eq!(parse("kai-slot 2\nrelease_seq 1\nepoch 1\n"), None);
        assert_eq!(parse("kai-slot 1\nepoch 1\n"), None);
        assert_eq!(parse("kai-slot 1\nrelease_seq -1\nepoch 1\n"), None);
        assert_eq!(parse(""), None);
    }
}
