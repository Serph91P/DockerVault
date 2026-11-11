//! Lossless JSON token ownership using Serde's raw and byte-string modes.
//! Escaped lone surrogates are retained, never replaced with valid-Unicode data.
//! Serde validates grammar; an iterative span walk formats without subtree reparse.
use crate::encoding::python_json_compact;
use serde::{Deserialize, Deserializer, Serialize, de::Visitor};
use serde_json::{Value, value::RawValue};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonError {
    Invalid,
    Cancelled,
}
impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid or incompatible JSON")
    }
}
impl std::error::Error for JsonError {}
type Result<T> = std::result::Result<T, JsonError>;
use JsonError::Invalid;
// No Debug: raw portable records can contain plaintext credentials.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct PythonJson(Box<RawValue>);
impl PythonJson {
    pub fn parse(input: &[u8]) -> Result<Self> {
        let input = std::str::from_utf8(input).map_err(|_| Invalid)?;
        RawValue::from_string(input.to_owned())
            .map(Self)
            .map_err(|_| Invalid)
    }
    pub fn as_str(&self) -> &str {
        self.0.get()
    }
    pub fn canonical(&self, cancel: &dyn Fn() -> bool) -> Result<Vec<u8>> {
        format(&self.0, true, false, cancel)
    }
    pub fn python(&self, cancel: &dyn Fn() -> bool) -> Result<String> {
        String::from_utf8(format(&self.0, false, true, cancel)?).map_err(|_| Invalid)
    }
}
// Standard serde_json byte mode yields WTF-8, combining valid surrogate pairs.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Bytes(Vec<u8>);
impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct ByteVisitor;
        impl Visitor<'_> for ByteVisitor {
            type Value = Bytes;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON string")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> std::result::Result<Bytes, E> {
                Ok(Bytes(v.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(
                self,
                v: Vec<u8>,
            ) -> std::result::Result<Bytes, E> {
                Ok(Bytes(v))
            }
        }
        d.deserialize_byte_buf(ByteVisitor)
    }
}
fn append_scalar_string(output: &mut Vec<u8>, text: &str) {
    let encoded = python_json_compact(&Value::String(text.to_owned()));
    output.extend_from_slice(&encoded.as_bytes()[1..encoded.len() - 1]);
}
fn quote(output: &mut Vec<u8>, mut bytes: &[u8]) -> Result<()> {
    output.push(b'"');
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                append_scalar_string(output, text);
                break;
            }
            Err(error) => {
                let prefix = error.valid_up_to();
                append_scalar_string(
                    output,
                    std::str::from_utf8(&bytes[..prefix]).map_err(|_| Invalid)?,
                );
                bytes = &bytes[prefix..];
                if bytes.len() < 3
                    || bytes[0] != 0xed
                    || !(0xa0..=0xbf).contains(&bytes[1])
                    || !(0x80..=0xbf).contains(&bytes[2])
                {
                    return Err(Invalid);
                }
                let code = 0xd000 | (u32::from(bytes[1] & 0x3f) << 6) | u32::from(bytes[2] & 0x3f);
                output.extend_from_slice(format!("\\u{code:04x}").as_bytes());
                bytes = &bytes[3..];
            }
        }
    }
    output.push(b'"');
    Ok(())
}
#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}
fn whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}
struct Tokens<'a> {
    text: &'a str,
    ends: BTreeMap<usize, usize>,
}
impl<'a> Tokens<'a> {
    fn new(text: &'a str, cancel: &dyn Fn() -> bool) -> Result<Self> {
        let mut ends = BTreeMap::new();
        let mut stack = Vec::new();
        let mut string = None;
        let mut escaped = false;
        // Index only boundaries in already validated JSON, not a second grammar.
        // This avoids RawValue repeatedly scanning a deeply nested subtree.
        for (i, byte) in text.bytes().enumerate() {
            if i % 16384 == 0 && cancel() {
                return Err(JsonError::Cancelled);
            }
            if let Some(start) = string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    ends.insert(start, i + 1);
                    string = None;
                }
            } else {
                match byte {
                    b'"' => string = Some(i),
                    b'[' | b'{' => stack.push(i),
                    b']' | b'}' => {
                        ends.insert(stack.pop().ok_or(Invalid)?, i + 1);
                    }
                    _ => {}
                }
            }
        }
        if string.is_some() || !stack.is_empty() {
            return Err(Invalid);
        }
        Ok(Self { text, ends })
    }
    fn skip(&self, mut cursor: usize, end: usize) -> usize {
        while cursor < end && whitespace(self.text.as_bytes()[cursor]) {
            cursor += 1;
        }
        cursor
    }
    fn value(&self, start: usize) -> Result<Span> {
        let byte = *self.text.as_bytes().get(start).ok_or(Invalid)?;
        let end = if matches!(byte, b'[' | b'{' | b'"') {
            *self.ends.get(&start).ok_or(Invalid)?
        } else {
            self.text.as_bytes()[start..]
                .iter()
                .position(|b| whitespace(*b) || matches!(b, b',' | b']' | b'}'))
                .map_or(self.text.len(), |length| start + length)
        };
        if end <= start {
            return Err(Invalid);
        }
        Ok(Span { start, end })
    }
    fn get(&self, span: Span) -> Result<&'a str> {
        self.text.get(span.start..span.end).ok_or(Invalid)
    }
    fn object(
        &self,
        span: Span,
        sorted: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<(Bytes, Span)>> {
        let mut entries = Vec::new();
        let end = span.end.checked_sub(1).ok_or(Invalid)?;
        let mut cursor = self.skip(span.start + 1, end);
        while cursor < end {
            if cancel() {
                return Err(JsonError::Cancelled);
            }
            if !entries.is_empty() {
                if self.text.as_bytes()[cursor] != b',' {
                    return Err(Invalid);
                }
                cursor = self.skip(cursor + 1, end);
            }
            let key = self.value(cursor)?;
            if self.text.as_bytes()[key.start] != b'"' {
                return Err(Invalid);
            }
            let key_text = serde_json::from_str::<Bytes>(self.get(key)?).map_err(|_| Invalid)?;
            cursor = self.skip(key.end, end);
            if self.text.as_bytes().get(cursor) != Some(&b':') {
                return Err(Invalid);
            }
            cursor = self.skip(cursor + 1, end);
            let value = self.value(cursor)?;
            if value.end > end {
                return Err(Invalid);
            }
            entries.push((key_text, value));
            cursor = self.skip(value.end, end);
        }
        // WTF-8 byte order is Python code-point order, including lone surrogates.
        let mut keys: Vec<_> = entries.iter().map(|(key, _)| key).collect();
        keys.sort_unstable();
        if keys.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(Invalid);
        }
        drop(keys);
        if sorted {
            entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        }
        Ok(entries)
    }
}
enum Frame {
    Array {
        cursor: usize,
        end: usize,
        first: bool,
    },
    Object {
        entries: std::vec::IntoIter<(Bytes, Span)>,
        first: bool,
    },
}
fn format(
    root: &RawValue,
    sorted: bool,
    spaced: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<u8>> {
    let tokens = Tokens::new(root.get(), cancel)?;
    let mut output = Vec::new();
    let mut frames = Vec::new();
    let mut next = Some(Span {
        start: 0,
        end: root.get().len(),
    });
    loop {
        if cancel() {
            return Err(JsonError::Cancelled);
        }
        if let Some(span) = next.take() {
            let text = tokens.get(span)?;
            match text.as_bytes().first().copied().ok_or(Invalid)? {
                b'[' => {
                    output.push(b'[');
                    frames.push(Frame::Array {
                        cursor: span.start + 1,
                        end: span.end - 1,
                        first: true,
                    });
                }
                b'{' => {
                    let entries = tokens.object(span, sorted, cancel)?;
                    output.push(b'{');
                    frames.push(Frame::Object {
                        entries: entries.into_iter(),
                        first: true,
                    });
                }
                b'"' => quote(
                    &mut output,
                    &serde_json::from_str::<Bytes>(text).map_err(|_| Invalid)?.0,
                )?,
                b't' | b'f' | b'n' => output.extend_from_slice(text.as_bytes()),
                _ => {
                    let float = text.contains(['.', 'e', 'E']);
                    if !float && text.trim_start_matches('-').len() > 4300 {
                        return Err(Invalid);
                    }
                    let value: Value = serde_json::from_str(text).map_err(|_| Invalid)?;
                    if !value.is_number() || (float && !value.as_f64().is_some_and(f64::is_finite))
                    {
                        return Err(Invalid);
                    }
                    output.extend_from_slice(python_json_compact(&value).as_bytes());
                }
            }
        }
        loop {
            if cancel() {
                return Err(JsonError::Cancelled);
            }
            let Some(frame) = frames.last_mut() else {
                return Ok(output);
            };
            match frame {
                Frame::Array { cursor, end, first } => {
                    *cursor = tokens.skip(*cursor, *end);
                    if *cursor == *end {
                        output.push(b']');
                        frames.pop();
                        continue;
                    }
                    if !*first {
                        if tokens.text.as_bytes().get(*cursor) != Some(&b',') {
                            return Err(Invalid);
                        }
                        *cursor = tokens.skip(*cursor + 1, *end);
                        output.push(b',');
                        if spaced {
                            output.push(b' ');
                        }
                    }
                    let span = tokens.value(*cursor)?;
                    if span.end > *end {
                        return Err(Invalid);
                    }
                    *cursor = span.end;
                    *first = false;
                    next = Some(span);
                    break;
                }
                Frame::Object { entries, first } => {
                    let Some((key, span)) = entries.next() else {
                        output.push(b'}');
                        frames.pop();
                        continue;
                    };
                    if !*first {
                        output.push(b',');
                        if spaced {
                            output.push(b' ');
                        }
                    }
                    quote(&mut output, &key.0)?;
                    output.push(b':');
                    if spaced {
                        output.push(b' ');
                    }
                    *first = false;
                    next = Some(span);
                    break;
                }
            }
        }
    }
}
