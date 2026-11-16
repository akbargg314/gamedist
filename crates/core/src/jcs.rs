//! RFC 8785 (JCS) canonical JSON, restricted to integer numbers.
//!
//! Signed GameDist objects never contain fractional numbers, which removes the
//! only hard part of JCS (ECMAScript number formatting). Floats are rejected.

use crate::{Error, Result};
use serde_json::Value;

const MAX_SAFE_INT: u64 = (1 << 53) - 1;

/// Serialise `value` canonically: no whitespace, object keys sorted by UTF-16
/// code units, minimal string escaping, integers only.
pub fn to_canonical_string(value: &Value) -> Result<String> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

fn write_value(value: &Value, out: &mut String) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                if u > MAX_SAFE_INT {
                    return Err(Error::invalid("canonical json", "integer exceeds 2^53-1"));
                }
                out.push_str(&u.to_string());
            } else if let Some(i) = n.as_i64() {
                if i.unsigned_abs() > MAX_SAFE_INT {
                    return Err(Error::invalid("canonical json", "integer exceeds 2^53-1"));
                }
                out.push_str(&i.to_string());
            } else {
                return Err(Error::invalid(
                    "canonical json",
                    "non-integer numbers are not allowed in signed objects",
                ));
            }
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&map[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{09}' => out.push_str("\\t"),
            '\u{0A}' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\u{0D}' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys_and_strips_whitespace() {
        let v: Value =
            serde_json::from_str(r#"{ "b": [1, 2, {"z": null, "a": true}], "a": "x" }"#).unwrap();
        assert_eq!(
            to_canonical_string(&v).unwrap(),
            r#"{"a":"x","b":[1,2,{"a":true,"z":null}]}"#
        );
    }

    #[test]
    fn rfc8785_string_and_key_ordering_vectors() {
        // From RFC 8785 §3.2.3: keys sort by UTF-16 code units.
        let v = json!({
            "\u{20ac}": "Euro Sign",
            "\r": "Carriage Return",
            "\u{fb33}": "Hebrew Letter Dalet With Dagesh",
            "1": "One",
            "\u{1f600}": "Emoji: Grinning Face",
            "\u{80}": "Control",
            "\u{f6}": "Latin Small Letter O With Diaeresis"
        });
        let s = to_canonical_string(&v).unwrap();
        let order: Vec<usize> = [
            "Carriage Return",
            "One",
            "Control",
            "Latin Small",
            "Euro Sign",
            "Emoji",
            "Hebrew",
        ]
        .iter()
        .map(|needle| s.find(needle).unwrap())
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{s}");
        assert!(s.starts_with(r#"{"\r":"#));

        let v = json!("\u{1}\u{1f}\"\\/\u{7f}é\n");
        assert_eq!(
            to_canonical_string(&v).unwrap(),
            "\"\\u0001\\u001f\\\"\\\\/\u{7f}é\\n\""
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(
            to_canonical_string(&json!([0, -1, 9007199254740991u64])).unwrap(),
            "[0,-1,9007199254740991]"
        );
        assert!(to_canonical_string(&json!(1.5)).is_err());
        assert!(to_canonical_string(&json!(9007199254740992u64)).is_err());
        assert!(to_canonical_string(&json!(-9007199254740992i64)).is_err());
    }

    #[test]
    fn independent_of_input_key_order() {
        let a: Value = serde_json::from_str(r#"{"x":1,"y":{"b":2,"a":3}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"y":{"a":3,"b":2},"x":1}"#).unwrap();
        assert_eq!(
            to_canonical_string(&a).unwrap(),
            to_canonical_string(&b).unwrap()
        );
    }
}
