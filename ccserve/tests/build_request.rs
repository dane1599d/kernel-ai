//! `build <component> <version>` through the service, with its `Shm`: the
//! sources in, the artifact back over them, and an app's artifact as `pkg`'s
//! `local` takes it. Hostile requests are refused.

mod common;

use ccserve::build::shm;
use common::{Cc, DECLARE};
use kabi::live::MAX_ARTIFACT;
use kaipkg::generation::Time;
use kaipkg::memvol::MemVolume;
use kaipkg::sign::Key;
use promptcore::wire::parse_reply;

/// An app that asks for nothing: what `pkg` installs from `local`.
const APP: &str = "#include <app.h>
// SPDX-License-Identifier: MIT
uint32_t exports_kai_live_lifecycle_schema(void) { return 1; }
bool exports_kai_live_lifecycle_init(exports_kai_live_lifecycle_start_error_t *err) { return true; }
bool exports_kai_live_lifecycle_migrate(app_list_u8_t *state, uint32_t from_schema,
                                        exports_kai_live_lifecycle_start_error_t *err) { return true; }
bool exports_kai_live_lifecycle_save(uint32_t to_schema, app_list_u8_t *ret) {
    ret->ptr = 0;
    ret->len = 0;
    return true;
}
uint32_t exports_kai_live_lifecycle_health(void) { return 0; }
uint32_t exports_kai_live_lifecycle_self_test(void) { return 0; }
void exports_kai_app_events_on_event(exports_kai_app_events_event_t *e) {
    exports_kai_app_events_event_free(e);
}
";

const NOW: Time = Time(1_793_000_000);

/// An `Shm` holding the bundle of `files`.
fn shm_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut shm = vec![0u8; shm::BYTES];
    shm::put(&mut shm, &livecore::artifact::bundle(files)).unwrap();
    shm
}

/// `message` with `shm`: the reply's records as strings.
fn build(cc: &mut Cc, message: &str, shm: Option<&mut [u8]>) -> Vec<String> {
    let reply = cc.service.handle_with(message.as_bytes(), shm, &cc.clock);
    parse_reply(&reply)
        .unwrap()
        .iter()
        .map(common::show)
        .collect()
}

/// The artifact a build of `files` writes back, and its reply.
fn built(cc: &mut Cc, name: &str, version: &str, files: &[(&str, &[u8])]) -> (Vec<u8>, String) {
    let mut shm = shm_of(files);
    let reply = build(cc, &format!("build {name} {version}"), Some(&mut shm));
    let [text] = &reply[..] else {
        panic!("{reply:?}")
    };
    let artifact = shm::take(&shm, MAX_ARTIFACT).unwrap().to_vec();
    (artifact, text.clone())
}

#[test]
fn an_app_built_through_the_shm_comes_back_over_its_sources() {
    let mut cc = Cc::open();
    let (artifact, text) = built(&mut cc, "greeter", "0.0.1", &[("main.c", APP.as_bytes())]);
    let parsed = livecore::artifact::parse(&artifact).unwrap();
    assert_eq!(parsed.manifest.world, "app:greeter/greeter@0.0.1");
    assert_eq!(parsed.manifest.licence, "MIT");
    let expected = format!(
        "text app {} built-by cc {} source {}",
        artifact.len(),
        ccserve::build::XCC_COMMIT,
        parsed.manifest.source
    );
    assert_eq!(text, expected);
    // The session is untouched.
    assert_eq!(cc.value("1 + 1"), "i64 2");
}

#[test]
fn a_component_is_built_as_one_from_its_header() {
    let mut cc = Cc::open();
    let source = APP
        .replace("#include <app.h>", "#include \"component.h\"")
        .replace("app_list_u8_t", "component_list_u8_t");
    let source = source.split("void exports_kai_app").next().unwrap();
    let (artifact, text) = built(&mut cc, "parts", "0.0.4", &[("parts.c", source.as_bytes())]);
    assert!(text.starts_with("text component "), "{text}");
    let parsed = livecore::artifact::parse(&artifact).unwrap();
    assert_eq!(parsed.manifest.version, "0-0-4");
}

/// What `pkg` does with it (#414): published into `local`, signed by the
/// device key, and checked as the image's repository is. A component's is not.
#[test]
fn local_publishes_and_checks_an_apps_artifact() {
    let mut cc = Cc::open();
    let (artifact, _) = built(&mut cc, "greeter", "0.0.1", &[("main.c", APP.as_bytes())]);
    let mut vol = MemVolume::default();
    let key = Key::from_seed([7; 32]);
    let published =
        kaipkg::local::publish(&mut vol, &key, &artifact, &[], kaipkg::PROVIDED, NOW).unwrap();
    assert_eq!(published.name, "greeter");
    let checked = kaipkg::local::check(&mut vol, &key, kaipkg::PROVIDED).unwrap();
    assert!(checked.refused.is_empty(), "{:?}", checked.refused);
    let next = kaipkg::local::next(&mut vol, &key, kaipkg::PROVIDED, "greeter").unwrap();
    assert_eq!(next.to_string(), "0.0.2");

    let source = APP
        .replace("#include <app.h>", "#include <component.h>")
        .replace("app_list_u8_t", "component_list_u8_t");
    let source = source.split("void exports_kai_app").next().unwrap();
    let (component, _) = built(&mut cc, "parts", "0.0.1", &[("parts.c", source.as_bytes())]);
    let refused = kaipkg::local::publish(&mut vol, &key, &component, &[], kaipkg::PROVIDED, NOW);
    assert!(refused.is_err());
}

#[test]
fn a_build_without_its_shm_or_with_a_bad_one_is_refused() {
    let mut cc = Cc::open();
    assert_eq!(
        build(&mut cc, "build greeter 0.0.1", None),
        ["err 0 build brings its sources in an Shm"]
    );
    let mut empty = vec![0u8; shm::BYTES];
    let reply = build(&mut cc, "build greeter 0.0.1", Some(&mut empty));
    assert_eq!(reply, ["err 0 not a kai-source 1 bundle"]);
    let mut long = vec![0u8; shm::BYTES];
    long[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    let reply = build(&mut cc, "build greeter 0.0.1", Some(&mut long));
    assert_eq!(reply, ["err 0 the Shm holds more than a build takes"]);
    let mut paths = shm_of(&[("../main.c", APP.as_bytes())]);
    let reply = build(&mut cc, "build greeter 0.0.1", Some(&mut paths));
    assert!(reply[0].starts_with("err 0 "), "{reply:?}");
    let mut version = shm_of(&[("main.c", APP.as_bytes())]);
    let reply = build(&mut cc, "build greeter 01.0.1", Some(&mut version));
    assert!(reply[0].starts_with("err 0 "), "{reply:?}");
}

#[test]
fn a_source_that_does_not_compile_is_named() {
    let mut cc = Cc::open();
    let mut shm = shm_of(&[("main.c", b"#include <app.h>\nint f( {\n")]);
    let reply = build(&mut cc, "build greeter 0.0.1", Some(&mut shm));
    assert!(reply[0].starts_with("err 0 main.c: "), "{reply:?}");
}

/// A build stops at its deadline as an entry does, and a run waiting on an
/// effect is still waiting after it.
#[test]
fn a_build_meets_the_deadline_and_leaves_a_waiting_run() {
    let mut cc = Cc::open();
    cc.ok(DECLARE);
    let need = cc.eval("add_one(41)");
    assert!(need[0].starts_with("need "), "{need:?}");
    let id = need[0].split(' ').nth(1).unwrap().to_string();
    cc.clock
        .step_ns
        .set(ccserve::LINE_NS * ccserve::service::BUILD_LINES);
    let mut shm = shm_of(&[("main.c", APP.as_bytes())]);
    assert_eq!(
        build(&mut cc, "build greeter 0.0.1", Some(&mut shm)),
        ["trap deadline"]
    );
    cc.clock.step_ns.set(0);
    let resumed = cc.send(&format!("resume {id} ok\n42"));
    assert_eq!(resumed[0], "value i64 42", "{resumed:?}");
}
