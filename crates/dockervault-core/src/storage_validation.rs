//! Source-compatible repair hints for stored provider configuration, not network
//! authorization, connectivity, SSRF protection or a transport URL parser.
use serde_json::Value;
use std::net::Ipv6Addr;

pub fn required_fields(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "local" => &["base_path"],
        "ssh" | "sftp" => &["host", "username", "base_path"],
        "webdav" => &["webdav_url", "base_path"],
        "s3" => &["s3_bucket", "s3_access_key", "s3_secret_key"],
        "ftp" => &["host", "username", "password", "base_path"],
        "rclone" => &["rclone_remote", "base_path"],
        _ => return None,
    })
}
fn whitespace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}
// All non-ASCII Unicode16 scalars whose NFKC introduces a netloc delimiter.
const NFKC_DELIMITERS: [u32; 19] = [
    8263, 8264, 8265, 8448, 8449, 8453, 8454, 10868, 65043, 65046, 65109, 65110, 65119, 65131,
    65283, 65295, 65306, 65311, 65312,
];
fn bracket_host(host: &str) -> bool {
    if host.starts_with(['v', 'V']) {
        let Some((version, tail)) = host[1..].split_once('.') else {
            return false;
        };
        return !version.is_empty()
            && version.bytes().all(|b| b.is_ascii_hexdigit())
            && !tail.is_empty();
    }
    let address = if let Some((address, scope)) = host.split_once('%') {
        if scope.is_empty() || scope.contains('%') {
            return false;
        }
        address
    } else {
        host
    };
    address.parse::<Ipv6Addr>().is_ok()
}
fn endpoint_metadata(value: &str) -> bool {
    // Match urllib.parse framing. Its permissive hostname is deliberately NOT
    // passed to networking: actual providers retain their own validated parser.
    let cleaned: String = value
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let cleaned = cleaned.trim_start_matches(|c| c <= '\u{20}');
    let Some((scheme, rest)) = cleaned.split_once(':') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let Some(rest) = rest.strip_prefix("//") else {
        return false;
    };
    let netloc = rest.split(['/', '?', '#']).next().unwrap();
    if netloc
        .chars()
        .any(|c| NFKC_DELIMITERS.contains(&u32::from(c)))
    {
        return false;
    }
    if netloc.contains('[') != netloc.contains(']') {
        return false;
    }
    let hostinfo = netloc.rsplit('@').next().unwrap();
    let (host, port) = if let Some((before, bracketed)) = hostinfo.split_once('[') {
        if !before.is_empty() {
            return false;
        }
        let Some((host, tail)) = bracketed.split_once(']') else {
            return false;
        };
        if !tail.is_empty() && !tail.starts_with(':') {
            return false;
        }
        (host, tail.strip_prefix(':').unwrap_or(""))
    } else {
        hostinfo.split_once(':').unwrap_or((hostinfo, ""))
    };
    if netloc.contains('[') && !bracket_host(host) {
        return false;
    }
    // Accepted CPython's default int-string bound includes leading-zero digits.
    if !port.is_empty()
        && (port.len() > 4300
            || !port.bytes().all(|b| b.is_ascii_digit())
            || port.parse::<u16>().is_err())
    {
        return false;
    }
    !host.is_empty()
}
/// These redacted hints intentionally retain the accepted provider's legacy
/// semantics; an empty list is not a successful connection or a security check.
pub fn storage_errors(values: &Value) -> Vec<String> {
    let kind = values
        .get("storage_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let Some(required) = required_fields(kind) else {
        return vec![
            "Unknown storage type; select a supported provider to repair this record".to_owned(),
        ];
    };
    let mut errors = Vec::new();
    for field in std::iter::once(&"name").chain(required) {
        if values
            .get(*field)
            .and_then(Value::as_str)
            .is_none_or(|s| s.trim_matches(whitespace).is_empty())
        {
            errors.push(format!(
                "{field} is required for {kind}; edit this storage to repair it"
            ));
        }
    }
    if values.get("enabled").is_none_or(Value::is_null) {
        errors.push("enabled must be true or false".to_owned());
    }
    if values
        .get("port")
        .is_some_and(|v| !v.is_null() && !v.as_i64().is_some_and(|n| (1..=65535).contains(&n)))
    {
        errors.push("port must be between 1 and 65535".to_owned());
    }
    for field in ["webdav_url", "s3_endpoint_url"] {
        if ((field == "webdav_url" && kind == "webdav")
            || (field == "s3_endpoint_url" && kind == "s3"))
            && values
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty() && !endpoint_metadata(s))
        {
            errors.push(format!(
                "{field} must be an http(s) URL with a valid hostname and port"
            ));
        }
    }
    errors
}
