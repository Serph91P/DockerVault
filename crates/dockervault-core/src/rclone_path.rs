//! The pinned rclone connection-string grammar; parsing never creates a filesystem.
use indexmap::IndexMap;
use serde::Serialize;
use unicode_general_category::{GeneralCategory as Category, get_general_category};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReferenceError;
impl std::fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid rclone reference")
    }
}
impl std::error::Error for ReferenceError {}

// Inline overrides can contain credentials, so deliberately do not implement Debug.
#[derive(Serialize)]
pub struct Reference {
    pub name: Option<String>,
    pub overrides: IndexMap<String, String>,
    pub path: String,
}
fn base(c: char) -> bool {
    matches!(c, '_' | '.' | '+' | '@')
        || matches!(
            get_general_category(c),
            Category::UppercaseLetter
                | Category::LowercaseLetter
                | Category::TitlecaseLetter
                | Category::ModifierLetter
                | Category::OtherLetter
                | Category::DecimalNumber
                | Category::LetterNumber
                | Category::OtherNumber
        )
}
pub fn valid_name(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let Some((position, _)) = value.char_indices().find(|(_, c)| !base(*c)) else {
        return true;
    };
    if position == 0 {
        return false;
    }
    let tail = &value[position..];
    // After the initial base run the source requires a separator AND a suffix;
    // a hyphen can be the suffix, but a final space never can.
    tail.len() >= 2
        && !tail.ends_with(' ')
        && tail.chars().all(|c| base(c) || matches!(c, ' ' | '-'))
}
fn local(value: &str) -> Reference {
    Reference {
        name: None,
        overrides: IndexMap::new(),
        path: value.to_owned(),
    }
}
pub fn parse(value: &str) -> Result<Reference, ReferenceError> {
    if value.is_empty() || value.contains('\0') {
        return Err(ReferenceError);
    }
    let bytes = value.as_bytes();
    if !bytes.contains(&b':') {
        return Ok(local(value));
    }
    let inline = value.starts_with(':');
    let mut index = usize::from(inline);
    while index < bytes.len() && !matches!(bytes[index], b':' | b',') {
        if matches!(bytes[index], b'/' | b'\\') {
            return if inline {
                Err(ReferenceError)
            } else {
                Ok(local(value))
            };
        }
        index += 1;
    }
    let name = &value[..index];
    if index == bytes.len() || !valid_name(name.strip_prefix(':').unwrap_or(name)) {
        return Err(ReferenceError);
    }
    let mut overrides = IndexMap::new();
    if bytes[index] == b':' {
        return Ok(Reference {
            name: Some(name.to_owned()),
            overrides,
            path: value[index + 1..].to_owned(),
        });
    }
    index += 1;
    while index < bytes.len() {
        let start = index;
        while index < bytes.len() && !matches!(bytes[index], b'=' | b':' | b',') {
            index += 1;
        }
        let parameter = &value[start..index];
        if index == bytes.len()
            || parameter.is_empty()
            || !parameter
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.'))
        {
            return Err(ReferenceError);
        }
        let mut delimiter = bytes[index];
        index += 1;
        let item = if delimiter == b'=' {
            let item = if index < bytes.len() && matches!(bytes[index], b'\'' | b'"') {
                let quote = bytes[index];
                index += 1;
                let mut output = Vec::new();
                let mut closed = false;
                while index < bytes.len() {
                    let c = bytes[index];
                    index += 1;
                    if c != quote {
                        output.push(c);
                    } else if index < bytes.len() && bytes[index] == quote {
                        output.push(quote);
                        index += 1;
                    } else {
                        closed = true;
                        break;
                    }
                }
                if !closed || index == bytes.len() || !matches!(bytes[index], b':' | b',') {
                    return Err(ReferenceError);
                }
                String::from_utf8(output).map_err(|_| ReferenceError)?
            } else {
                let start = index;
                while index < bytes.len() && !matches!(bytes[index], b':' | b',') {
                    index += 1;
                }
                if index == bytes.len() {
                    return Err(ReferenceError);
                }
                value[start..index].to_owned()
            };
            delimiter = bytes[index];
            index += 1;
            item
        } else {
            "true".to_owned()
        };
        overrides.insert(parameter.to_owned(), item);
        if delimiter == b':' {
            return Ok(Reference {
                name: Some(name.to_owned()),
                overrides,
                path: value[index..].to_owned(),
            });
        }
    }
    Err(ReferenceError)
}
