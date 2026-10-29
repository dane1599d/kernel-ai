//! The shell's answer to a line, as the kernel terminal reads it
//! (`shellcore::Reply::to_wire`): `ok <text>`, `err <text>`, `confirm <id>
//! <text>`, `menu <id> ...` or `more`.

use termcore::menu::Menu;

#[derive(Debug, PartialEq, Eq)]
pub enum Wire<'a> {
    /// A bracket is still open: the next line continues the entry.
    More,
    Ok(&'a str),
    Err(&'a str),
    /// A script waits for a yes. The trusted path answers, not this terminal.
    Confirm {
        id: u64,
        text: &'a str,
    },
    /// A call waits on the Allow / Deny menu.
    Menu(Menu),
    /// A `menu` that does not decode or cannot be shown exactly: its id, if it
    /// reads, so the call can be denied.
    BadMenu(Option<u64>),
    Malformed,
}

pub fn parse(wire: &str) -> Wire<'_> {
    if wire == "more" {
        return Wire::More;
    }
    if let Some(rest) = wire.strip_prefix("menu ") {
        return match Menu::decode(wire) {
            Ok(menu) => Wire::Menu(menu),
            Err(_) => Wire::BadMenu(
                rest.split([' ', '\n'])
                    .next()
                    .and_then(|id| id.parse().ok()),
            ),
        };
    }
    let (kind, rest) = wire.split_once(' ').unwrap_or((wire, ""));
    match kind {
        "ok" => Wire::Ok(rest),
        "err" => Wire::Err(rest),
        "confirm" => match rest.split_once(' ') {
            Some((id, text)) => id
                .parse()
                .map_or(Wire::Malformed, |id| Wire::Confirm { id, text }),
            None => Wire::Malformed,
        },
        _ => Wire::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termcore::menu::Origin;

    #[test]
    fn reads_each_kind() {
        assert_eq!(parse("more"), Wire::More);
        assert_eq!(parse("ok 42"), Wire::Ok("42"));
        assert_eq!(parse("ok"), Wire::Ok(""));
        assert_eq!(parse("err no such command"), Wire::Err("no such command"));
        assert_eq!(
            parse("confirm 7 script 7: trash it"),
            Wire::Confirm {
                id: 7,
                text: "script 7: trash it"
            }
        );
    }

    #[test]
    fn a_menu_decodes_or_names_its_id() {
        let call = "process_stop(\"x\")";
        let question = alloc::format!("Stop {call}? It has no undo.");
        let wire = Menu::encode(5, Origin::Asked, &question, "", "", call);
        assert!(matches!(parse(&wire), Wire::Menu(m) if m.id == 5));
        assert_eq!(parse("menu 9 asked\nq"), Wire::BadMenu(Some(9)));
        assert_eq!(parse("menu x"), Wire::BadMenu(None));
    }

    #[test]
    fn anything_else_is_malformed() {
        for wire in ["", "okay", "confirm", "confirm x text", "confirm 3", "OK 1"] {
            assert_eq!(parse(wire), Wire::Malformed, "{wire:?}");
        }
    }
}
