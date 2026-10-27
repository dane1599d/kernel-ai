//! OLPC canonical JSON, as securesystemslib's `encode_canonical` writes it for
//! TUF: no whitespace, object keys in code-point order, integers only, and in
//! strings only `"` and `\` escaped, every other character as itself, control
//! characters included. A fraction has no canonical form and is refused, so
//! metadata holding one is.
//!
//! serde_json's compact output escapes control characters, and every PEM key
//! holds newlines, so serde_json does not give these bytes for any root with a
//! PEM key: that is why this is its own writer (spike S08-3).

use crate::{Error, malformed};
use alloc::vec::Vec;
use serde_json::Value;

/// `value`'s canonical bytes.
pub fn canonical(value: &Value) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    write(value, &mut out)?;
    Ok(out)
}

fn write(value: &Value, out: &mut Vec<u8>) -> Result<(), Error> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            out.extend_from_slice(alloc::format!("{n}").as_bytes());
        }
        Value::Number(n) => return malformed(alloc::format!("{n} is not an integer")),
        Value::String(s) => string(s, out)?,
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            // serde_json's map is a BTreeMap, which orders keys by their bytes:
            // for UTF-8 that is code-point order, as Python's sorted() gives.
            out.push(b'{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                string(key, out)?;
                out.push(b':');
                write(item, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn string(s: &str, out: &mut Vec<u8>) -> Result<(), Error> {
    out.push(b'"');
    for b in s.bytes() {
        if matches!(b, b'"' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b'"');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sort_and_only_quotes_and_backslashes_escape() {
        let value = json!({ "b": [1, true, null, -7], "a": "x\"y\\z/é\u{7f}", "B": {} });
        let got = canonical(&value).unwrap();
        assert_eq!(
            core::str::from_utf8(&got).unwrap(),
            "{\"B\":{},\"a\":\"x\\\"y\\\\z/é\u{7f}\",\"b\":[1,true,null,-7]}"
        );
    }

    #[test]
    fn fractions_have_no_form_and_control_characters_stay_raw() {
        assert!(canonical(&json!({ "a": 1.5 })).is_err());
        assert_eq!(canonical(&json!(["a\nb\t"])).unwrap(), b"[\"a\nb\t\"]");
    }

    #[test]
    fn serde_json_agrees_only_without_control_characters() {
        let value = json!({ "signed": { "_type": "root", "version": 3, "keys": {} }, "z": [""] });
        assert_eq!(
            canonical(&value).unwrap(),
            serde_json::to_vec(&value).unwrap()
        );
        let pem =
            json!({ "public": "-----BEGIN PUBLIC KEY-----\nMFkw\n-----END PUBLIC KEY-----\n" });
        assert_ne!(canonical(&pem).unwrap(), serde_json::to_vec(&pem).unwrap());
    }
}
