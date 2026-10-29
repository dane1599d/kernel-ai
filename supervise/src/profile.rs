//! `profile <pid> <class> <pages|->`: the shell asks the supervisor to set one
//! process's CPU class and page quota for a resource profile (P3 task 12). The
//! supervisor holds SUPERVISE, which PROC_CLASS_SET and PROC_QUOTA_SET need, and
//! takes this only from the shell. The kernel's floors refuse a protected
//! process, whatever the profile says.
//!
//! The answer is `ok <old class> <old pages|->`, what the process had before, so
//! `undo` can put it back; `-` where the quota was left as it was.

use alloc::format;
use alloc::string::String;

/// One process's settings, or the ones it had: class 0 is normal, 1 background
/// (PROC_CLASS_SET's numbers), and a page quota or none to leave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Set {
    pub pid: u64,
    pub class: u64,
    pub pages: Option<u64>,
}

const WORD: &str = "profile";

fn class_word(class: u64) -> Option<&'static str> {
    match class {
        0 => Some("normal"),
        1 => Some("background"),
        _ => None,
    }
}

fn class_of(word: &str) -> Option<u64> {
    match word {
        "normal" => Some(0),
        "background" => Some(1),
        _ => None,
    }
}

fn pages_word(pages: Option<u64>) -> String {
    pages.map_or(String::from("-"), |p| format!("{p}"))
}

fn pages_of(word: &str) -> Option<Option<u64>> {
    match word {
        "-" => Some(None),
        _ => number(word).map(Some),
    }
}

/// Digits only, no sign, no leading zero.
fn number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    (digits && (text == "0" || !text.starts_with('0')))
        .then(|| text.parse().ok())
        .flatten()
}

impl Set {
    /// The request's text; `None` for a class with no word.
    pub fn encode(&self) -> Option<String> {
        let class = class_word(self.class)?;
        Some(format!(
            "{WORD} {} {class} {}",
            self.pid,
            pages_word(self.pages)
        ))
    }

    /// A request read back; `None` for anything out of shape.
    pub fn parse(text: &str) -> Option<Self> {
        let mut words = text.split(' ');
        if words.next()? != WORD {
            return None;
        }
        let set = Self {
            pid: number(words.next()?)?,
            class: class_of(words.next()?)?,
            pages: pages_of(words.next()?)?,
        };
        words.next().is_none().then_some(set)
    }
}

/// The supervisor's `ok` answer: what `pid` had before.
pub fn encode_before(before: Set) -> String {
    let class = class_word(before.class).unwrap_or("normal");
    format!("ok {class} {}", pages_word(before.pages))
}

/// The answer read back: what the process had, or the supervisor's `err` text.
pub fn parse_before(pid: u64, text: &str) -> Result<Set, String> {
    let mut words = text.split(' ');
    match words.next() {
        Some("ok") => {}
        Some("err") => return Err(String::from(text.trim_start_matches("err "))),
        _ => return Err(format!("the supervisor's answer was malformed: {text}")),
    }
    let parsed = (|| {
        let set = Set {
            pid,
            class: class_of(words.next()?)?,
            pages: pages_of(words.next()?)?,
        };
        words.next().is_none().then_some(set)
    })();
    parsed.ok_or_else(|| format!("the supervisor's answer was malformed: {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_and_its_answer_read_back() {
        let set = Set {
            pid: 31,
            class: 1,
            pages: Some(65536),
        };
        let text = set.encode().unwrap();
        assert_eq!(text, "profile 31 background 65536");
        assert_eq!(Set::parse(&text), Some(set));
        let before = Set {
            pid: 31,
            class: 0,
            pages: None,
        };
        assert_eq!(encode_before(before), "ok normal -");
        assert_eq!(parse_before(31, "ok normal -"), Ok(before));
        assert_eq!(
            parse_before(31, "err Permission"),
            Err(String::from("Permission"))
        );
    }

    #[test]
    fn anything_out_of_shape_is_refused() {
        for bad in [
            "profile",
            "profile 31",
            "profile 31 firm -",
            "profile 031 normal -",
            "profile 31 normal 12x",
            "profile 31 normal - extra",
            "profiles 31 normal -",
            "profile -1 normal -",
        ] {
            assert_eq!(Set::parse(bad), None, "{bad}");
        }
        assert!(parse_before(1, "ok fast -").is_err());
        assert!(parse_before(1, "maybe").is_err());
        let no_word = Set {
            pid: 1,
            class: 2,
            pages: None,
        };
        assert_eq!(no_word.encode(), None);
    }
}
