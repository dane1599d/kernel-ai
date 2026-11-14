//! Volume and sound requests get one fixed answer (founder test 2): Kernel AI
//! has no audio driver yet, so the model must not offer or invent a volume
//! change. Code answers before the model, so no model input changes. Only a
//! sound word used as an audio command or noun counts ("turn the volume up",
//! "mute", "play some audio"); "does this sound right", "disk volume" and
//! "open sound.txt" go to the model as before.

/// The answer to a request about sound.
pub(crate) const NO_SOUND: &str = "There's no sound yet: Kernel AI has no audio driver, so I can't play sound or change the volume.";

/// Verbs that set a level: with "volume" or "sound", an audio command.
const LEVEL_VERBS: &[&str] = &[
    "turn", "set", "raise", "lower", "increase", "decrease", "adjust", "change",
];

/// Words that are audio commands on their own.
const AUDIO_COMMANDS: &[&str] = &["mute", "unmute", "louder", "quieter"];

/// What "play" plays when it is sound.
const PLAYED: &[&str] = &["sound", "sounds", "audio", "music", "song", "songs"];

/// The most words a request about sound has.
const MAX_WORDS: usize = 8;

/// [`NO_SOUND`] if `line` is a short audio command or question.
pub(crate) fn answer(line: &str) -> Option<&'static str> {
    let tokens: alloc::vec::Vec<&str> = line.split_whitespace().collect();
    // A path or file name is a name, never a sound word: "open sound.txt".
    if tokens.len() > MAX_WORDS || tokens.iter().any(|t| is_name(t)) {
        return None;
    }
    let words: alloc::vec::Vec<alloc::string::String> = tokens
        .iter()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    let level = has(LEVEL_VERBS) && has(&["volume", "sound"]);
    let command = has(AUDIO_COMMANDS);
    let play = words.first().is_some_and(|w| w == "play") && has(PLAYED);
    let volume_up = pair(&words, "volume", &["up", "down"]);
    let working = pair(&words, "sound", &["working", "broken"])
        || pair(&words, "audio", &["working", "broken"]);
    (level || command || play || volume_up || working).then_some(NO_SOUND)
}

/// Whether `token` names a file or path: it has a `.` inside, a `/`, or a
/// leading `~`.
fn is_name(token: &str) -> bool {
    let inner = token.trim_end_matches(['.', '?', '!', ',']);
    inner.contains('/') || inner.starts_with('~') || inner.contains('.')
}

/// Whether `first` is followed at once by one of `then`.
fn pair(words: &[alloc::string::String], first: &str, then: &[&str]) -> bool {
    words
        .windows(2)
        .any(|w| w[0] == first && then.contains(&w[1].as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_audio_command_or_question_gets_the_fixed_answer() {
        for line in [
            "turn the volume up",
            "set the volume to 50",
            "lower the sound",
            "mute",
            "unmute please",
            "make it louder",
            "play some audio",
            "play music",
            "volume up",
            "is the sound working?",
        ] {
            assert_eq!(answer(line), Some(NO_SOUND), "{line}");
        }
    }

    #[test]
    fn other_meanings_of_sound_and_volume_go_to_the_model() {
        for line in [
            "sound out this word",
            "does this sound right",
            "is my plan sound",
            "volume of a sphere radius 2",
            "how much disk volume is free",
            "make a volume calculator app",
            "open sound.txt",
            "delete audio.log",
            "read ~/sound",
            "show /home/user/music/volume",
            "make a note called sound",
            "show me the cpus",
        ] {
            assert_eq!(answer(line), None, "{line}");
        }
    }
}
