//! "What each app can do" (ADR 0001 D8): the grants in `kai-grants 1`, grouped
//! by app and said in plain words, each with the id Settings revokes it by.
//! Settings shows grants; it never makes one. Revoking goes through the grant
//! rows (A-core's), which log it and undo it.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kaigrants::{From, Grant, Grants, Kind, Life};

/// One thing an app may do, and the grant it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub grant: u32,
    /// "Read and write /home/user/notes".
    pub what: String,
    /// "You chose it in a file picker", "Until restart" and so on.
    pub why: String,
}

/// One app and everything its grants let it do, in grant order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAccess {
    pub app: String,
    pub access: Vec<Access>,
}

/// The rights words, joined as a person would say them: `read+write` is
/// "Read and write", three or more take commas.
fn rights(words: &[String]) -> String {
    let mut out = String::new();
    for (i, word) in words.iter().enumerate() {
        let sep = match (i, words.len() - i) {
            (0, _) => "",
            (_, 1) => " and ",
            _ => ", ",
        };
        out.push_str(sep);
        out.push_str(word);
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::from("Use"),
    }
}

fn what(grant: &Grant) -> String {
    let rights = rights(&grant.rights);
    match grant.kind {
        Kind::Dir => format!("{rights} the folder {}", grant.object),
        Kind::File => format!("{rights} the file {}", grant.object),
        Kind::Host => format!("Connect to {}", grant.object),
    }
}

fn why(grant: &Grant) -> String {
    let from = match grant.from {
        From::Install => String::from("Given when it was installed"),
        From::Picker => String::from("You chose it in a file picker"),
        From::Request(entry) => format!("You allowed it when asked (log entry {entry})"),
        From::Settings => String::from("You allowed it in Settings"),
    };
    match grant.life {
        Life::Persistent => from,
        Life::Session => format!("{from}, until restart"),
    }
}

/// Every app with at least one grant, by name.
pub fn by_app(grants: &Grants) -> Vec<AppAccess> {
    let mut apps: Vec<AppAccess> = Vec::new();
    for grant in &grants.grants {
        let access = Access {
            grant: grant.id,
            what: what(grant),
            why: why(grant),
        };
        match apps.binary_search_by(|a| a.app.as_str().cmp(&grant.app)) {
            Ok(at) => apps[at].access.push(access),
            Err(at) => apps.insert(
                at,
                AppAccess {
                    app: grant.app.clone(),
                    access: alloc::vec![access],
                },
            ),
        }
    }
    apps
}
