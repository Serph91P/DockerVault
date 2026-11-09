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
        let path = path.trim_start_matches('/');
        !self.exclude.iter().any(|p| p.accepts(path))
            && (self.include.is_empty() || self.include.iter().any(|p| p.accepts(path)))
    }

    pub fn is_parent_of_included(&self, directory: &str) -> bool {
        let directory = format!(
            "{}/",
            directory.trim_start_matches('/').trim_end_matches('/')
        );
        self.include.iter().any(|p| {
            p.literal_prefix.starts_with(&directory) || directory.starts_with(&p.literal_prefix)
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

    fn accepts(&self, path: &str) -> bool {
        if path.starts_with(&self.directory_prefix) {
            return true;
        }
        let chars: Vec<char> = path.chars().collect();
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

impl Token {
    fn accepts(&self, c: char) -> bool {
        match self {
            Self::Any | Self::Star => true,
            Self::Literal(expected) => c == *expected,
            Self::Class { negated, ranges } => {
                ranges.iter().any(|&(start, end)| start <= c && c <= end) != *negated
            }
        }
    }
}
