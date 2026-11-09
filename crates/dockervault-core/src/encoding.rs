//! Byte-compatible Python json.dumps defaults for persisted metadata/identity.
use serde_json::Value;
use std::fmt::Write;

pub fn python_string(value: &str) -> String {
    let encoded = serde_json::to_string(value).expect("a string serializes as JSON");
    let mut result = String::new();
    for character in encoded.chars() {
        if character > '~' {
            for unit in character.encode_utf16(&mut [0; 2]) {
                write!(result, "\\u{unit:04x}").expect("write to string");
            }
        } else {
            result.push(character);
        }
    }
    result
}
pub fn python_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == f64::INFINITY {
        return "Infinity".into();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".into();
    }
    let text = format!("{value:?}");
    if let Some((mantissa, exponent)) = text.split_once('e') {
        let exponent: i32 = exponent.parse().expect("native finite float exponent");
        format!("{mantissa}e{exponent:+03}")
    } else {
        text
    }
}
pub fn python_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::String(value) => python_string(value),
        Value::Number(value) => {
            let literal = value.to_string();
            if literal.contains(['.', 'e', 'E']) {
                python_float(literal.parse().expect("valid JSON numeric literal"))
            } else if literal == "-0" {
                "0".into()
            } else {
                literal
            }
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", python_string(key), python_json(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
