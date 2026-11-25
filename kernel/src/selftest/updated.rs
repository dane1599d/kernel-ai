//! The `updated.<key>=` words a test boot hands on to `updated`'s plan line.

use super::{ActCase, SelfTest};
use alloc::format;
use alloc::string::String;

/// The `updated.<key>=` words an `update` boot passes on to `updated`'s line:
/// those its window and restart read, and the online step's `retries` (how many
/// times a network stop is tried again) and `into` (the slot to write from R)
/// (`user/src/bin/updated`). A key a case passes that is not here is dropped
/// without a word, so `xtask`'s host tests check every case's `updated.` words
/// against this list.
const UPDATED_TEST_WORDS: [&str; 5] = ["restart", "window", "heads_up_s", "retries", "into"];

/// The words after `updated`'s plan line in a test boot: `start=demand`
/// ([`kabi::plan::ON_DEMAND`]), so it holds no memory until the shell's first
/// model request, except in `update`, which runs a step of the update service
/// at boot (`updated=`, `check` by default; a refusal restarts into the
/// running slot) and takes `updated.<word>` as a word for the keys in
/// [`UPDATED_TEST_WORDS`], as `restart=window`.
/// The default boot starts it at boot (`kabi::plan::system`), and so do the
/// cases in [`updated_at_boot`].
pub(super) fn updated_words(test: SelfTest) -> String {
    if updated_at_boot(test) {
        return String::new();
    }
    if test != SelfTest::Update {
        return String::from(kabi::plan::ON_DEMAND);
    }
    let step = crate::cmdline()
        .split_whitespace()
        .find_map(|w| w.strip_prefix("updated="))
        .unwrap_or("check");
    let mut words = format!(" after=restart step={step}");
    if step == "online" {
        // `updated` reaches the HTTPS mock's three hosts on port 443 through a
        // `NetDest` of its own (P5 task 10). Its pages hold a 2 MiB chunk, the
        // read-back buffers and the TLS records on top of `kabi::plan::UPDATED`'s.
        let hosts = crate::pkg_tests::ONLINE_HOSTS;
        words.push_str(&format!(
            " pages=6144 online=test netdest=proto:tcp,ports:443,hosts:{hosts}"
        ));
    }
    if step == "download" {
        // The model download (P5 task 9): the HTTPS mock's model host, the
        // storage host it redirects to, and the repository host with the
        // role's chunk lists; its pages hold a 2 MiB chunk and the TLS
        // records on top of `kabi::plan::UPDATED`'s. The held `NetDest` lets
        // it read the bundle's test CA (`netroots/1.der`).
        words.push_str(
            " pages=6144 fetch=test netdest=proto:tcp,ports:443,hosts:models.kai.test+assets.kai.test+updates.kai.test",
        );
    }
    // Only the words `updated` reads: a plan word such as `netdest=` or
    // `caps=` from the command line would widen its line (the manager's review
    // of #779).
    for word in crate::cmdline()
        .split_whitespace()
        .filter_map(|w| w.strip_prefix("updated."))
        .filter(|w| {
            w.split_once('=')
                .is_some_and(|(key, _)| UPDATED_TEST_WORDS.contains(&key))
        })
    {
        words.push(' ');
        words.push_str(word);
    }
    words
}

/// The cases whose first model list would otherwise wait on `updated`'s start:
/// the agent's (`app-models`) and the window's (`ui-models`, `ui-models-use`).
fn updated_at_boot(test: SelfTest) -> bool {
    matches!(
        test,
        SelfTest::Act(ActCase::AppModels) | SelfTest::UiModels | SelfTest::UiModelsUse
    )
}
