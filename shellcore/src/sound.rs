//! Volume and sound requests get one fixed answer (founder test 2): Kernel AI
//! has no audio driver yet, so the model must not offer or invent a volume
//! change. Code answers before the model, so no model input changes.

/// The answer to a request about sound.
pub(crate) const NO_SOUND: &str = "There's no sound yet: Kernel AI has no audio driver, so I can't play sound or change the volume.";

/// Words that ask about sound.
const SOUND_WORDS: &[&str] = &[
    "volume", "mute", "unmute", "louder", "quieter", "sound", "audio", "speaker", "speakers",
];

/// Words that make the line about something else ("the sound file").
const ELSEWHERE: &[&str] = &[
    "file", "files", "note", "notes", "folder", "named", "called",
];

/// The most words a request about sound has.
const MAX_WORDS: usize = 8;

/// [`NO_SOUND`] if `line` is a short request about sound or volume.
pub(crate) fn answer(line: &str) -> Option<&'static str> {
    let words: alloc::vec::Vec<alloc::string::String> = line
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let about_sound = words.iter().any(|w| SOUND_WORDS.contains(&w.as_str()));
    let elsewhere = words.iter().any(|w| ELSEWHERE.contains(&w.as_str()));
    (about_sound && !elsewhere && words.len() <= MAX_WORDS).then_some(NO_SOUND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_about_sound_gets_the_fixed_answer() {
        for line in [
            "turn the volume up",
            "mute",
            "make it louder",
            "is the sound working?",
            "play some audio",
        ] {
            assert_eq!(answer(line), Some(NO_SOUND), "{line}");
        }
        for line in [
            "open the sound file",
            "make a note called sound",
            "show me the cpus",
            "the volume of files in my home folder is too big for the disk today",
        ] {
            assert_eq!(answer(line), None, "{line}");
        }
    }
}
