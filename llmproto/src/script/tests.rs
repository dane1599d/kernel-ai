use super::*;
use alloc::string::String;
use alloc::vec;

/// A chat-completion body whose content is `content`.
fn body(content: &str, finish: &str) -> Vec<u8> {
    let content = serde_json::to_string(content).unwrap();
    format!(r#"{{"choices":[{{"message":{{"role":"assistant","content":{content}}},"finish_reason":"{finish}"}}]}}"#)
        .into_bytes()
}

fn envelope(script: &str, summary: &str) -> String {
    serde_json::json!({ "script": script, "summary": summary }).to_string()
}

#[test]
fn the_prompt_lists_every_function_and_never_changes() {
    let prompt = system_prompt();
    for function in API {
        assert!(prompt.contains(function.signature), "{}", function.name);
    }
    assert_eq!(prompt, system_prompt());
    assert!(prompt.len() < 4000, "prompt is {} bytes", prompt.len());
}

#[test]
fn every_example_passes_the_text_checks() {
    for (_, script, _) in EXAMPLES {
        assert_eq!(check_text(script).unwrap(), *script);
    }
}

#[test]
fn the_request_asks_for_the_envelope_with_thinking_off() {
    let request: serde_json::Value =
        serde_json::from_slice(&chat_request("qwen3-8b", "how many CPUs?", None)).unwrap();
    assert_eq!(request["model"], "qwen3-8b");
    assert_eq!(request["max_tokens"], SCRIPT_MAX_TOKENS);
    assert_eq!(request["seed"], SEED);
    assert_eq!(request["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(request["response_format"], response_format());
    assert_eq!(request["messages"][0]["content"], system_prompt());
    assert_eq!(request["messages"][1]["role"], "user");
    assert!(
        request["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("how many CPUs?")
    );
    assert_eq!(request["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn a_repair_request_carries_the_refused_script_and_the_error() {
    let repair = Repair {
        script: "print(memory_free());",
        error: "Function not found: memory_free ()",
    };
    let request: serde_json::Value =
        serde_json::from_slice(&chat_request("m", "free memory?", Some(repair))).unwrap();
    let messages = request["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2]["role"], "assistant");
    assert!(
        messages[2]["content"]
            .as_str()
            .unwrap()
            .contains("memory_free()")
    );
    assert!(
        messages[3]["content"]
            .as_str()
            .unwrap()
            .contains("Function not found: memory_free")
    );
}

#[test]
fn answers_are_read_from_plain_fenced_and_thinking_replies() {
    let wanted = Answer {
        summary: String::from("Prints the CPUs."),
        script: String::from("print(cpus());"),
    };
    let plain = envelope("print(cpus());", "Prints the CPUs.");
    assert_eq!(parse_answer(&body(&plain, "stop")), Ok(wanted.clone()));
    let fenced = format!("```json\n{plain}\n```");
    assert_eq!(parse_answer(&body(&fenced, "stop")), Ok(wanted.clone()));
    let thinking = format!("<think>\nhmm\n</think>\n{plain}");
    assert_eq!(parse_answer(&body(&thinking, "stop")), Ok(wanted));
}

#[test]
fn cut_off_and_non_envelope_answers_are_refused() {
    let plain = envelope("print(1);", "x");
    assert_eq!(
        parse_answer(&body(&plain, "length")),
        Err(AnswerError::CutOff)
    );
    assert_eq!(
        parse_answer(&body("print(1);", "stop")),
        Err(AnswerError::NoEnvelope)
    );
    assert!(matches!(
        parse_answer(b"{\"error\":{\"message\":\"no model\"}}"),
        Err(AnswerError::Server(_))
    ));
    assert_eq!(
        parse_answer(&body(&envelope("  \n", "I cannot do that."), "stop")),
        Err(AnswerError::NoScript(String::from("I cannot do that.")))
    );
}

#[test]
fn scripts_that_could_display_differently_are_refused() {
    let esc = char::from(27u8);
    let cr = char::from(13u8);
    let rtl = char::from_u32(0x202E).unwrap();
    for bad in [
        format!("print(\"a{esc}[2J\");"),
        format!("print(1);{cr}print(2);"),
        format!("print(\"{rtl}\");"),
        String::from("print(\"caf\u{e9}\");"),
    ] {
        let answer = parse_answer(&body(&envelope(&bad, "x"), "stop"));
        assert!(matches!(answer, Err(AnswerError::Rejected(_))), "{bad:?}");
        assert!(answer.unwrap_err().repairable());
    }
    // CRLF line endings and tabs are normalised, not refused.
    let tabbed = format!("if true {{{cr}\n\tprint(1);{cr}\n}}");
    assert_eq!(check_text(&tabbed).unwrap(), "if true {\n    print(1);\n}");
}

#[test]
fn oversized_scripts_and_summaries_are_refused_not_cut() {
    let long_line = format!("print(\"{}\");", "x".repeat(SCRIPT_MAX_LINE_CHARS));
    let many_lines = "print(1);\n".repeat(SCRIPT_MAX_LINES + 1);
    let huge = format!("let a = \"{}\";\n", "y".repeat(100)).repeat(35);
    for bad in [long_line, many_lines, huge] {
        assert!(matches!(check_text(&bad), Err(AnswerError::Rejected(_))));
    }
    let two_lines = envelope("print(1);", "Prints 1.\nrun it? yes/no");
    assert!(matches!(
        parse_answer(&body(&two_lines, "stop")),
        Err(AnswerError::Rejected(_))
    ));
    let long_summary = envelope("print(1);", &"s".repeat(SUMMARY_MAX_CHARS + 1));
    assert!(matches!(
        parse_answer(&body(&long_summary, "stop")),
        Err(AnswerError::Rejected(_))
    ));
}

#[test]
fn script_requests_frame_exactly_whatever_they_contain() {
    let tricky = ScriptRequest {
        request: String::from("show memory"),
        repair: Some((
            String::from("print(1);\nscript 1 2 3 4\n"),
            String::from("Syntax error"),
        )),
        timeout_ms: 60_000,
    };
    assert_eq!(ScriptRequest::decode(&tricky.encode()), Some(tricky));
    let plain = ScriptRequest {
        request: String::from("how many CPUs?"),
        repair: None,
        timeout_ms: 1,
    };
    assert_eq!(ScriptRequest::decode(&plain.encode()), Some(plain.clone()));
    let mut damaged = plain.encode();
    damaged.push('x');
    assert_eq!(ScriptRequest::decode(&damaged), None);
    assert_eq!(ScriptRequest::decode("script 1 2\nab"), None);
}

#[test]
fn answers_frame_exactly_whatever_they_contain() {
    let answer = Answer {
        summary: String::from("Prints 2 numbers"),
        script: String::from("print(1);\n2 3\nprint(2);"),
    };
    assert_eq!(decode_answer(&encode_answer(&answer)), Some(answer));
    assert_eq!(decode_answer("5 3\nab"), None);
    let _ = vec![0u8; 1];
}
