use super::*;
use agentabi::{Facts, Inverse, Owner, RetType, Tier};
use rhai::Engine;

/// A content read, for the labels only P2's rows will hand out.
const NOTE_READ: Action = Action {
    name: "note_read",
    doc: "the text of a note",
    args: &[],
    facts: Facts::LOCAL_READ,
    inverse: Inverse::Read,
    cues: &[],
    returns: Returns::Content("note \"todo\""),
    ret: RetType::Data,
    owner: Owner::Shell,
    needs: 0,
    min_tier: Tier::Run,
    old_name: None,
};

static ROWS: [Action; 4] = [
    agentabi::SET_POLICY,
    agentabi::GET_POLICY,
    agentabi::CPUS,
    NOTE_READ,
];

fn plan(script: &str) -> Plan {
    let mut engine = Engine::new_raw();
    engine.set_strict_variables(false);
    let ast = engine
        .compile(script)
        .unwrap_or_else(|e| panic!("{e}: {script}"));
    analyse(&ast, &ROWS)
}

/// The one `set_policy` site: its argument and control.
fn switch(script: &str) -> (Val, Label) {
    let plan = plan(script);
    let sites: Vec<&Site> = plan
        .sites
        .iter()
        .filter(|s| s.name == "set_policy")
        .collect();
    assert_eq!(sites.len(), 1, "{script}: {plan:?}");
    (sites[0].args[0].1.clone(), sites[0].control)
}

const NOTE: Label = Label::Content("note \"todo\"");

#[test]
fn a_literal_call_is_found_however_it_is_written() {
    for script in [
        "set_policy(\"rr\");",
        "\"rr\".set_policy();",
        "print(`${set_policy(\"rr\")}`);",
        "let p = \"rr\"; set_policy(p);",
    ] {
        assert_eq!(
            switch(script),
            (Val::literal("rr"), Label::Literal),
            "{script}"
        );
    }
}

#[test]
fn a_name_in_a_comment_or_a_string_is_no_call() {
    let plan = plan("// set_policy(\"rr\")\nprint(\"set_policy cpus\");\nlet set_policy_x = 1;");
    assert!(plan.sites.is_empty(), "{plan:?}");
}

#[test]
fn values_carry_where_they_came_from() {
    assert_eq!(switch("set_policy(get_policy());").0.label, Label::System);
    assert_eq!(switch("let n = note_read(); set_policy(n);").0.label, NOTE);
    assert_eq!(
        switch("let n = note_read(); set_policy(n[0]);").0.label,
        NOTE
    );
    assert_eq!(
        switch("let p = \"rr\"; p += note_read(); set_policy(p);")
            .0
            .label,
        NOTE
    );
    assert_eq!(
        switch("let a = [\"rr\"]; a[0] = get_policy(); set_policy(a[0]);")
            .0
            .label,
        Label::System
    );
}

#[test]
fn what_decides_a_call_is_its_control() {
    assert_eq!(
        switch("if cpus() > 2 { set_policy(\"rr\"); }").1,
        Label::System
    );
    assert_eq!(
        switch("if note_read() == \"x\" { set_policy(\"rr\"); }").1,
        NOTE
    );
    assert_eq!(
        switch("for c in note_read() { set_policy(\"rr\"); }").1,
        NOTE
    );
    assert_eq!(
        switch("switch note_read() { 1 => set_policy(\"rr\"), _ => () }").1,
        NOTE
    );
    assert_eq!(
        switch("note_read() == \"x\" && set_policy(\"rr\");").1,
        NOTE
    );
    // An early exit decides what comes after it.
    assert_eq!(
        switch("if note_read() == \"x\" { return; }\nset_policy(\"rr\");").1,
        NOTE
    );
    assert_eq!(
        switch("if cpus() > 2 { throw \"no\"; }\nset_policy(\"rr\");").1,
        Label::System
    );
}

#[test]
fn a_value_assigned_under_control_carries_it() {
    let (arg, control) =
        switch("let p = \"rr\";\nif note_read() == \"x\" { p = \"interactive\"; }\nset_policy(p);");
    assert_eq!((arg.label, arg.text, control), (NOTE, None, Label::Literal));
    let (arg, _) = switch("let p = \"rr\";\nif cpus() > 2 { p = \"rr\"; }\nset_policy(p);");
    assert_eq!(
        arg,
        Val {
            label: Label::System,
            text: Some("rr".into())
        }
    );
}

#[test]
fn a_loop_is_walked_until_its_variables_settle() {
    let (arg, _) = switch(
        "let p = \"rr\";\nlet q = \"rr\";\nfor i in 0..3 {\n set_policy(p);\n p = q;\n q = get_policy();\n}",
    );
    assert_eq!(arg.label, Label::System);
}

#[test]
fn an_inner_let_does_not_hide_the_outer_value() {
    let (arg, _) = switch("let x = get_policy();\n{ let x = \"rr\"; }\nset_policy(x);");
    assert_eq!(arg.label, Label::System);
}

#[test]
fn a_catch_is_decided_by_what_the_try_met() {
    let (_, control) = switch("try { let n = note_read(); n[0]; } catch { set_policy(\"rr\"); }");
    assert_eq!(control, NOTE);
}

#[test]
fn allows_checks_the_literals_it_found() {
    let plan = plan("set_policy(\"rr\");\nset_policy(get_policy());\ncpus();");
    assert!(plan.allows("set_policy", &["rr"]));
    assert!(
        plan.allows("set_policy", &["interactive"]),
        "the get_policy site"
    );
    assert!(plan.allows("cpus", &[]));
    assert!(!plan.allows("get_policy", &["x"]));
    let only = super::tests::plan("set_policy(\"rr\");");
    assert!(!only.allows("set_policy", &["interactive"]));
    assert!(!only.allows("cpus", &[]));
    assert_eq!(plan.names(), ["set_policy", "get_policy", "cpus"]);
}
