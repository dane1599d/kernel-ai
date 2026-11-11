//! `kai:net@0.1.0`'s bindings (`wit/kai-net/http.wit`, world `http-client`):
//! the requests the fixture makes, and the answers the host gives back,
//! handed to the fixture.

use crate::{Fixture, Outcome};
use alloc::format;
use alloc::string::String;

wit_bindgen::generate!({
    world: "kai:net/http-client",
    path: "../../wit/kai-net",
    generate_all,
});

use exports::kai::net::responses::{Error, Guest as Responses, Response};
use kai::net::http::{Method, Request};

struct Bound;

/// Asks the host for `url`, whole, with GET; its ticket.
pub fn fetch(url: &str) -> u32 {
    kai::net::http::fetch(&Request {
        url: String::from(url),
        method: Method::Get,
        range: None,
    })
}

fn outcome(outcome: Result<Response, Error>) -> Outcome {
    match outcome {
        Ok(response) => Outcome::Response {
            status: response.status,
            url: response.url,
            body: response.body,
        },
        Err(error) => Outcome::Error(match error {
            Error::Refused(why) => format!("refused {why}"),
            Error::BadUrl(why) => format!("bad-url {why}"),
            Error::Tls(why) => format!("tls {why}"),
            Error::Http(why) => format!("http {why}"),
            Error::TooLarge => String::from("too-large"),
            Error::Timeout => String::from("timeout"),
            Error::Truncated => String::from("truncated"),
        }),
    }
}

impl Responses for Bound {
    fn on_response(ticket: u32, answer: Result<Response, Error>) {
        let told = compsdk::with::<Fixture, ()>(|app| app.answered(ticket, outcome(answer)));
        if told.is_none() {
            compsdk::log("net-fixture: an answer came before it started");
        }
    }
}

export!(Bound);
