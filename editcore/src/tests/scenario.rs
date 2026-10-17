use super::*;
use crate::command::{Command, parse};
use crate::scenario::{EDIT_LINES, STEPS, Step};
use crate::session::Session;
use crate::{EDITABLE, rules};

/// Each of the scenario's edits matches the real notes source exactly once, at the
/// line the scenario expects, and passes the edit rules. A change to notes that
/// moves them fails here, on the host, before anything boots.
#[test]
fn the_scenario_edits_land_on_the_real_notes_source() {
    let bundle = real_notes();
    for (edit, line) in EDIT_LINES {
        let mut session = Session::open("notes", "2", &bundle).unwrap();
        let Ok(Command::Substitute {
            line: None,
            old,
            new,
        }) = parse(edit)
        else {
            panic!("{edit} is not a substitution");
        };
        assert_eq!(session.substitute(None, old, new), Ok(line), "{edit}");
        assert_eq!(
            rules::check(&EDITABLE[0], &bundle, &session.to_bundle()),
            Ok(()),
            "{edit}"
        );
    }
}

#[test]
fn every_edit_line_in_the_scenario_parses() {
    for step in STEPS {
        if let Step::Edit(line, _) = step
            && !line.starts_with(':')
        {
            assert!(parse(line).is_ok(), "{line}");
        }
    }
}
