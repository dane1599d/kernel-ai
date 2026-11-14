//! The Models window's switches on their way to the shell and back (#556),
//! carried by `desktop` exactly as the Settings app's requests are
//! ([`crate::settings_wire`]), under their own words and in a queue of their
//! own:
//!
//! - from the window's `comphost`, `models put\n<lines>` → `ok`, each line
//!   `<ticket>\t<user 0|1>\tuse <sha256>`; any caller but [`MODELS_APP`] gets
//!   `err not the models app`;
//! - from it, `models results` → its answers;
//! - from the shell, `models take` → `none`, or `app models-window\n<lines>`;
//! - from the shell, `models done models-window\n<answers>` → `ok`;
//! - from the shell, `models back\n<lines>` → `ok`: requests it puts back.
//!
//! The window is `desktop`'s own record of the caller, never anything in a
//! request, and the shell checks the app and `user` again before it runs Kai's
//! `model_use` row.

use crate::settings_wire::Words;

/// The one process that may ask: the model manager's window, by its plan
/// line's name (`modelchan::asker::WINDOW`, which the shell pins).
pub const MODELS_APP: &str = "models-window";

/// The Models window's words.
pub const MODELS: Words = Words {
    word: "models ",
    put: "models put\n",
    results: "models results",
    take: "models take",
    done: "models done ",
    back: "models back\n",
    app: MODELS_APP,
    refused: "not the models app",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_wire::{
        Answer, Queue, Request, SETTINGS, SETTINGS_APP, encode_done_chunks_for, encode_take_for,
        parse_answers, parse_take,
    };
    use alloc::string::String;

    const SHA: &str = "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597";

    #[test]
    fn only_the_models_window_may_put_on_its_queue() {
        let mut queue = Queue::default();
        let line = alloc::format!("4\t1\tuse {SHA}");
        assert_eq!(
            queue.put_as(&MODELS, SETTINGS_APP, &line),
            Err("not the models app")
        );
        assert_eq!(
            queue.put_as(&MODELS, "shell", &line),
            Err("not the models app")
        );
        assert_eq!(queue.take(4096), None, "a refused put queues nothing");
        assert_eq!(queue.put_as(&MODELS, MODELS_APP, &line), Ok(()));
        let taken = queue.take(4096).unwrap();
        assert_eq!(
            taken,
            [Request {
                ticket: 4,
                user: true,
                text: alloc::format!("use {SHA}"),
            }]
        );
    }

    #[test]
    fn the_settings_queue_takes_nothing_from_the_models_window() {
        let mut settings = Queue::default();
        assert_eq!(
            settings.put(MODELS_APP, "1\t1\tprofile-set quiet"),
            Err("not the settings app")
        );
        assert_eq!(
            settings.put_as(&SETTINGS, MODELS_APP, "1\t1\tprofile-set quiet"),
            Err("not the settings app")
        );
    }

    #[test]
    fn a_take_and_its_answers_name_the_models_window_under_the_models_words() {
        let request = Request {
            ticket: 2,
            user: true,
            text: alloc::format!("use {SHA}"),
        };
        let take = encode_take_for(&MODELS, Some(core::slice::from_ref(&request)));
        assert_eq!(take, alloc::format!("app models-window\n2\t1\tuse {SHA}"));
        let (app, requests) = parse_take(&take).unwrap().unwrap();
        assert_eq!((app, requests), (MODELS_APP, alloc::vec![request]));

        let answers = [Answer::new(2, Ok(String::from("Done")))];
        let done = encode_done_chunks_for(&MODELS, MODELS_APP, &answers, 4096);
        assert_eq!(done, ["models done models-window\n2\tok\t4\nDone"]);
        let body = done[0].strip_prefix("models done models-window\n").unwrap();
        assert_eq!(parse_answers(body).unwrap(), answers);
    }

    #[test]
    fn the_two_apps_words_never_share_a_message() {
        for (s, m) in [
            (SETTINGS.put, MODELS.put),
            (SETTINGS.results, MODELS.results),
            (SETTINGS.take, MODELS.take),
            (SETTINGS.done, MODELS.done),
        ] {
            assert_ne!(s, m);
            assert!(s.starts_with(SETTINGS.word), "{s}");
            assert!(m.starts_with(MODELS.word), "{m}");
        }
        assert_ne!(SETTINGS.app, MODELS.app);
    }
}
