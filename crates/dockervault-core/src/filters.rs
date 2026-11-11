//! Published Linux Python fnmatch/include-prefix/exclude-wins semantics.

pub struct Filters {
    include: Vec<Pattern>,
    exclude: Vec<Pattern>,
}

impl Filters {
    pub fn new(include: &[String], exclude: &[String]) -> Self {
        Self {
            include: include.iter().map(|p| Pattern::new(p)).collect(),
            exclude: exclude.iter().map(|p| Pattern::new(p)).collect(),
        }
    }

    pub fn includes(&self, path: &str) -> bool {
        self.includes_bytes(path.as_bytes())
    }

    pub fn includes_bytes(&self, path: &[u8]) -> bool {
        let path = strip_slashes(path);
        !self.exclude.iter().any(|p| p.accepts(path))
            && (self.include.is_empty() || self.include.iter().any(|p| p.accepts(path)))
    }

    pub fn traverses_bytes(&self, directory: &[u8]) -> bool {
        let directory = strip_slashes(directory);
        !self.exclude.iter().any(|p| p.accepts(directory))
            && (self.include.is_empty()
                || self.include.iter().any(|p| p.accepts(directory))
                || self.parent_bytes(directory))
    }

    pub fn is_parent_of_included(&self, directory: &str) -> bool {
        self.parent_bytes(directory.as_bytes())
    }

    fn parent_bytes(&self, directory: &[u8]) -> bool {
        let directory = strip_slashes(directory);
        let end = directory
            .iter()
            .rposition(|&c| c != b'/')
            .map_or(0, |i| i + 1);
        let mut directory = directory[..end].to_vec();
        directory.push(b'/');
        self.include.iter().any(|p| {
            p.literal_prefix.as_bytes().starts_with(&directory)
                || directory.starts_with(p.literal_prefix.as_bytes())
        })
    }
}

struct Pattern {
    tokens: Vec<Token>,
    directory_prefix: String,
    literal_prefix: String,
}

enum Token {
    Star,
    Any,
    Literal(char),
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl Pattern {
    fn new(pattern: &str) -> Self {
        let pattern = pattern.trim_start_matches('/');
        let chars: Vec<char> = pattern.chars().collect();
        let mut tokens = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            i += 1;
            match c {
                '*' => {
                    if !matches!(tokens.last(), Some(Token::Star)) {
                        tokens.push(Token::Star);
                    }
                }
                '?' => tokens.push(Token::Any),
                '[' => {
                    let negated = chars.get(i) == Some(&'!');
                    let begin = i + usize::from(negated);
                    let mut end = begin + usize::from(chars.get(begin) == Some(&']'));
                    while end < chars.len() && chars[end] != ']' {
                        end += 1;
                    }
                    if end == chars.len() {
                        tokens.push(Token::Literal('['));
                        continue;
                    }
                    tokens.push(character_class(&chars[i..end]));
                    i = end + 1;
                }
                c => tokens.push(Token::Literal(c)),
            }
        }
        Self {
            tokens,
            directory_prefix: format!("{}/", pattern.trim_end_matches('*').trim_end_matches('/')),
            literal_prefix: pattern
                .split(['*', '?', '['])
                .next()
                .unwrap_or("")
                .to_owned(),
        }
    }

    fn accepts(&self, path: &[u8]) -> bool {
        if path.starts_with(self.directory_prefix.as_bytes()) {
            return true;
        }
        let chars = filesystem_characters(path);
        let mut matches = vec![false; chars.len() + 1];
        matches[0] = true;
        // One row of dynamic programming avoids recursive wildcard backtracking.
        for token in &self.tokens {
            if matches!(token, Token::Star) {
                for i in 1..matches.len() {
                    matches[i] |= matches[i - 1];
                }
            } else {
                for i in (1..matches.len()).rev() {
                    matches[i] = matches[i - 1] && token.accepts(chars[i - 1]);
                }
                matches[0] = false;
            }
        }
        matches[chars.len()]
    }
}

// Range normalization follows CPython fnmatch (Python license; see dependency notices).
fn character_class(chars: &[char]) -> Token {
    let mut chunks = Vec::<Vec<char>>::new();
    let mut cursor = 0;
    let mut seek = if chars.first() == Some(&'!') { 2 } else { 1 };
    while let Some(offset) = chars
        .get(seek..)
        .and_then(|rest| rest.iter().position(|&c| c == '-'))
    {
        let dash = seek + offset;
        chunks.push(chars[cursor..dash].to_vec());
        cursor = dash + 1;
        seek = dash + 3;
    }
    if cursor < chars.len() {
        chunks.push(chars[cursor..].to_vec());
    } else if let Some(last) = chunks.last_mut() {
        last.push('-');
    }
    for k in (1..chunks.len()).rev() {
        if let (Some(&left), Some(&right)) = (chunks[k - 1].last(), chunks[k].first())
            && left > right
        {
            let next = chunks.remove(k);
            chunks[k - 1].pop();
            chunks[k - 1].extend_from_slice(&next[1..]);
        }
    }
    let mut units = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        if index != 0 {
            units.push(('-', true));
        }
        units.extend(chunk.iter().map(|&c| (c, false)));
    }
    let negated = units.first() == Some(&('!', false));
    let mut j = usize::from(negated);
    let mut ranges = Vec::new();
    while j < units.len() {
        if j + 2 < units.len() && !units[j].1 && units[j + 1].1 && !units[j + 2].1 {
            ranges.push((units[j].0, units[j + 2].0));
            j += 3;
        } else {
            ranges.push((units[j].0, units[j].0));
            j += 1;
        }
    }
    Token::Class { negated, ranges }
}

fn strip_slashes(path: &[u8]) -> &[u8] {
    &path[path.iter().position(|&c| c != b'/').unwrap_or(path.len())..]
}

// Match Linux Python filesystem decoding without U+FFFD collisions or unsafe UTF-8.
fn filesystem_characters(mut bytes: &[u8]) -> Vec<u32> {
    let mut result = Vec::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                result.extend(text.chars().map(u32::from));
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                result.extend(
                    std::str::from_utf8(&bytes[..valid])
                        .expect("validated UTF-8 prefix")
                        .chars()
                        .map(u32::from),
                );
                bytes = &bytes[valid..];
                let invalid = error.error_len().unwrap_or(bytes.len());
                result.extend(bytes[..invalid].iter().map(|&c| 0xdc00 + u32::from(c)));
                bytes = &bytes[invalid..];
            }
        }
    }
    result
}

impl Token {
    fn accepts(&self, c: u32) -> bool {
        match self {
            Self::Any | Self::Star => true,
            Self::Literal(expected) => c == u32::from(*expected),
            Self::Class { negated, ranges } => {
                ranges
                    .iter()
                    .any(|&(start, end)| u32::from(start) <= c && c <= u32::from(end))
                    != *negated
            }
        }
    }
}
