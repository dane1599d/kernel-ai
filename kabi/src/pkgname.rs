//! A package's name: the one check every part of the system that takes a name
//! from outside it uses (ADR 0011 section 2). A name is 2 to 12 of `a-z`, `0-9`
//! and `-`, and starts with a letter, so it is also a safe path segment, a service
//! name and a plan-line word. The editor, `wasmhost`, `desktop` and the shell each
//! check a name before a build is asked for or run (and `cc` and `pkg` check it
//! again); they share this one, so the rule cannot drift between them.

/// The fewest bytes in a package's name.
pub const MIN_LEN: usize = 2;
/// The most bytes in a package's name.
pub const MAX_LEN: usize = 12;
/// The rule in words, to follow `"<name>" is not a package's name: `.
pub const RULE: &str = "2-12 of a-z, 0-9 and -, starting with a letter";

/// Whether `name` is a package's name.
pub fn is_valid(name: &str) -> bool {
    (MIN_LEN..=MAX_LEN).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_two_to_twelve_of_the_allowed_bytes_starting_with_a_letter() {
        for name in ["ab", "hello", "a1", "my-app", "hello-world1", "a--b"] {
            assert!(is_valid(name), "{name}");
        }
        assert_eq!("hello-world1".len(), MAX_LEN);
    }

    #[test]
    fn anything_else_is_not() {
        for name in [
            "",
            "a",
            "hello-world12",
            "1ab",
            "-ab",
            "Hello",
            "hello_x",
            "hello world",
            "he/llo",
            "he.llo",
            "../x",
            "héllo",
            "hello\n",
            "he\0llo",
        ] {
            assert!(!is_valid(name), "{name:?}");
        }
    }
}
