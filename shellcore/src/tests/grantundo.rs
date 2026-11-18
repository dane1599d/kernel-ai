//! A package's network grants through the real install and undo (ADR 0002
//! D3): an undo that takes a package out drops its grants, and no one else's;
//! an undo of an upgrade keeps them; a fresh install of a name pkg lacks drops
//! any left under it; and Kai asks pkg again after each.

use super::appgrants::{WEATHER, file};
use super::*;
use crate::ccbuild::CcBuilt;
use alloc::vec;
use kaigrants::Life;

const PROVENANCE: &str = "built-by cc 631e318432aeaaf72d6d2dfc93085766934314a2 source 5e1f";

/// The grants every test starts from: foo's own and its window's, and two
/// other apps'.
fn grants() -> String {
    file(&[
        ("foo", "api.foo.example", Life::Persistent),
        ("foo-window", "foo.example", Life::Session),
        ("weather", WEATHER, Life::Persistent),
        ("net-fixture", "a.kai.test", Life::Persistent),
    ])
}

/// The apps the file holds grants for now.
fn holders(host: &FakeHost) -> Vec<String> {
    let text = host.net_disk.borrow().clone().unwrap();
    crate::netgrant::NetGrants::load(&text)
        .unwrap()
        .all()
        .iter()
        .map(|g| g.app.clone())
        .collect()
}

fn installed(host: &FakeHost, package: &str) {
    host.installed
        .borrow_mut()
        .push((package.into(), "kai-actions 1".into()));
}

fn uninstalled(host: &FakeHost, package: &str) {
    host.installed.borrow_mut().retain(|(p, _)| p != package);
}

/// `cc build foo`, installed as generation 5, with `grants` on file after it.
fn built_foo() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for answer in ["0.0.3", "foo 0.0.3 generation 5 from 4"] {
        host.pkg_answers.borrow_mut().push_back(Ok(answer.into()));
    }
    *host.cc_built.borrow_mut() = Some(Ok(CcBuilt {
        app: true,
        bytes: 5_120,
        provenance: String::from(PROVENANCE),
        path: String::from("/home/user/c/foo.kai"),
    }));
    let reply = shell.eval("cc build foo");
    assert!(reply.ok && reply.text.contains("generation 5"), "{reply:?}");
    installed(&host, "foo");
    installed(&host, "weather");
    // The person allows foo's site after the install.
    *host.net_disk.borrow_mut() = Some(grants());
    (host, shell)
}

fn undo(host: &FakeHost, shell: &mut Shell) {
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("generation 6")));
    let reply = shell.eval("undo");
    assert_eq!(reply, ok("Undone: packages back to generation 4"));
}

#[test]
fn an_undo_that_takes_a_package_out_drops_its_grants_and_no_one_elses() {
    let (host, mut shell) = built_foo();
    host.calls
        .borrow_mut()
        .push_back(super::appgrants::fetch("api.foo.example", None));
    // Kai's memo holds pkg's "installed" for foo when the undo runs.
    use nethosts::Granted;
    let foo = nethosts::Host::parse("api.foo.example").unwrap();
    assert!(crate::appgrants::AppGrants::new(host.as_ref(), &shell.grant_memo).granted(&foo));
    uninstalled(&host, "foo");
    undo(&host, &mut shell);
    assert_eq!(holders(&host), ["weather", "net-fixture"]);
    let audit = host.audit.borrow().clone();
    assert!(
        audit
            .iter()
            .any(|a| a.ends_with("(foo on foo.example) dropped: the undo took foo out")),
        "{audit:?}"
    );
    // A fetch from foo's site right after asks.
    let reply = shell.eval("look it up online");
    assert!(reply.confirm.is_some(), "{reply:?}");
    assert!(host.net_seen.borrow().is_empty());
}

#[test]
fn an_undo_of_an_upgrade_keeps_the_packages_grants() {
    let (host, mut shell) = built_foo();
    // foo stays installed at its version before.
    undo(&host, &mut shell);
    assert_eq!(
        holders(&host),
        ["foo", "foo-window", "weather", "net-fixture"]
    );
}

#[test]
fn an_undo_pkg_cannot_answer_about_drops_and_says_so() {
    let (host, mut shell) = built_foo();
    *host.installed_down.borrow_mut() = Some("pkg did not answer".into());
    undo(&host, &mut shell);
    assert_eq!(holders(&host), ["weather", "net-fixture"]);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|a| a.contains("pkg could not say: pkg did not answer")),
        "{:?}",
        host.audit.borrow()
    );
}

#[test]
fn a_cc_build_of_a_new_name_drops_grants_left_under_it() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    *host.net_disk.borrow_mut() = Some(grants());
    for answer in ["0.0.1", "foo 0.0.1 generation 5 from 4"] {
        host.pkg_answers.borrow_mut().push_back(Ok(answer.into()));
    }
    *host.cc_built.borrow_mut() = Some(Ok(CcBuilt {
        app: true,
        bytes: 5_120,
        provenance: String::from(PROVENANCE),
        path: String::from("/home/user/c/foo.kai"),
    }));
    assert!(shell.eval("cc build foo").ok);
    assert_eq!(holders(&host), ["weather", "net-fixture"]);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|a| a.ends_with("dropped: foo was not installed before this install")),
        "{:?}",
        host.audit.borrow()
    );
}

#[test]
fn a_cc_build_pkg_cannot_answer_about_keeps_grants_and_says_so() {
    let host = FakeHost::new();
    *host.installed_down.borrow_mut() = Some("pkg did not answer".into());
    let mut shell = shell(&host);
    *host.net_disk.borrow_mut() = Some(grants());
    for answer in ["0.0.1", "foo 0.0.1 generation 5 from 4"] {
        host.pkg_answers.borrow_mut().push_back(Ok(answer.into()));
    }
    *host.cc_built.borrow_mut() = Some(Ok(CcBuilt {
        app: true,
        bytes: 5_120,
        provenance: String::from(PROVENANCE),
        path: String::from("/home/user/c/foo.kai"),
    }));
    assert!(shell.eval("cc build foo").ok);
    assert_eq!(holders(&host).len(), 4);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|a| a.starts_with("foo's old network grants, if any, were kept")),
        "{:?}",
        host.audit.borrow()
    );
}

#[test]
fn kais_app_install_of_a_new_name_drops_grants_left_under_it() {
    let (host, mut shell) = super::appinstall::built();
    *host.net_disk.borrow_mut() = Some(file(&[
        ("timer", "api.timer.example", Life::Persistent),
        ("weather", WEATHER, Life::Persistent),
    ]));
    let call = super::appinstall::call_for(&host);
    shell.eval("install timer");
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("timer 0.0.1 generation 5 from 4")));
    let once = host.mint(2, &call);
    assert!(shell.eval(&format!("answer 2 allow once={once}")).ok);
    assert_eq!(holders(&host), ["weather"]);
}

#[test]
fn an_install_makes_kai_ask_pkg_again() {
    let (host, mut shell) = built_foo();
    use nethosts::Granted;
    let foo = nethosts::Host::parse("api.foo.example").unwrap();
    assert!(crate::appgrants::AppGrants::new(host.as_ref(), &shell.grant_memo).granted(&foo));
    uninstalled(&host, "foo");
    // Within the memo's keep, pkg's old answer stands...
    assert!(crate::appgrants::AppGrants::new(host.as_ref(), &shell.grant_memo).granted(&foo));
    // ...until the undo forgets it.
    shell.local_removed("cc build bar 0.0.1");
    assert!(!crate::appgrants::AppGrants::new(host.as_ref(), &shell.grant_memo).granted(&foo));
    let _ = vec![0];
}
