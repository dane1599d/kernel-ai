//! Odd and hostile lines through the shell (P1 hardening): seeded random lines
//! from fragments of real requests, other scripts, control characters, nested
//! quotes and very long text. Nothing panics, every reply is a reply, and the
//! package-line parser reads every line as a package line or not, without
//! panicking.

use super::*;
use crate::pkg::line;

/// Marsaglia's xorshift64, as `testkit/hostile.rs` has it.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x % n as u64) as usize
    }
}

const FRAGMENTS: &[&str] = &[
    "switch",
    "to",
    "round",
    "robin",
    "stop",
    "build-3",
    "install",
    "notes",
    "apt",
    "sudo",
    "-y",
    "remove",
    "undo",
    "rules",
    "rules_remove",
    "new",
    "show",
    "log",
    "answer",
    "1",
    "allow",
    "once=",
    "rule=",
    "let",
    "x",
    "=",
    "5",
    ";",
    "print(x)",
    "fn",
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    "\"",
    "'",
    "\\",
    "`",
    "\u{0}",
    "\u{7}",
    "\u{1b}[31m",
    "\t",
    "\r",
    "\n",
    "\u{202e}",
    "переключи",
    "轮询",
    "🙂",
    "?",
    "!",
    "..",
    "/home/user",
    "../..",
    "%n",
    "cambia",
    "la",
    "política",
    "make",
    "it",
    "snappier",
    "do",
    "again",
    "the",
    "same",
];

fn line(rng: &mut Rng) -> String {
    let words = match rng.below(12) {
        0 => 800,
        _ => rng.below(12),
    };
    (0..words)
        .map(|_| FRAGMENTS[rng.below(FRAGMENTS.len())])
        .collect::<Vec<_>>()
        .join(if rng.below(3) == 0 { "" } else { " " })
}

#[test]
fn no_line_makes_the_shell_panic_and_each_gets_a_reply() {
    let mut rng = Rng(0x0dd_11e5);
    let host = FakeHost::new();
    let mut shell = super::shell(&host);
    for _ in 0..3_000 {
        let text = line(&mut rng);
        let reply = shell.eval(&text);
        // The wire form is always one of the shell's replies.
        let wire = reply.to_wire();
        assert!(
            ["ok", "err", "confirm", "menu", "more"]
                .iter()
                .any(|kind| wire.starts_with(kind)),
            "{text:?} -> {wire:?}"
        );
    }
}

#[test]
fn the_package_line_parser_reads_any_line() {
    let mut rng = Rng(0x9ac_4a9e);
    for _ in 0..20_000 {
        let text = line(&mut rng);
        if let Some(Ok(parsed)) = line::parse(&text) {
            // Whatever it took as a package name is one.
            let names = match &parsed.command {
                line::Command::Install(names) | line::Command::Remove(names) => names.clone(),
                _ => Vec::new(),
            };
            for name in names {
                assert!(
                    name.len() <= 40 && !name.contains(' '),
                    "{text:?}: {name:?}"
                );
            }
        }
    }
}
