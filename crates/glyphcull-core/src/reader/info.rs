//! INFO section decoder (SPEC.md §2.1): deterministic JSON — a single flat
//! object, keys sorted lexicographically, no whitespace, minimal escaping,
//! integer-only numbers. The parser is deliberately restricted to this
//! subset: unknown or wrong-typed keys are rejected with typed errors, so a
//! package can never smuggle ambiguous metadata into the runtime.

use crate::error::{Error, ErrorKind, Result};
use crate::limits::MAX_INFO_LEN;

/// INFO metadata (SPEC.md §2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// Must equal the header version (`1`).
    pub format_version: u32,
    /// The compiler name, e.g. `glyphcull-compiler`.
    pub generator: String,
    /// The semantic compiler version.
    pub generator_version: String,
    /// Hex SHA-256 of the normalized source input(s).
    pub source_digest: String,
    /// Hex: first 16 bytes of SHA-256 over the decoded content sections.
    pub document_id: String,
    /// The document title.
    pub title: Option<String>,
    /// A BCP 47 language tag.
    pub lang: Option<String>,
    /// CHNK record count.
    pub chunk_count: u32,
    /// STYL record count.
    pub style_count: u32,
    /// CONT payload count.
    pub content_count: u32,
    /// GLYF atlas count.
    pub atlas_count: u32,
    /// IMGS image count.
    pub image_count: u32,
}

/// Decode the INFO payload.
pub fn decode(payload: &[u8]) -> Result<Info> {
    if payload.len() as u64 > MAX_INFO_LEN {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("INFO payload {} bytes > {MAX_INFO_LEN}", payload.len()),
        ));
    }
    let text = std::str::from_utf8(payload)
        .map_err(|_| Error::new(ErrorKind::InvalidUtf8, "INFO payload is not valid UTF-8"))?;
    let mut parser = InfoJson::new(text);
    let mut keys: Vec<(String, JsonValue)> = Vec::new();
    parser.expect('{')?;
    let mut seen = std::collections::HashSet::new();
    if parser.peek() == Some('}') {
        parser.advance()?;
    } else {
        loop {
            let key = parser.string()?;
            if !seen.insert(key.clone()) {
                return Err(parser.error(format!("duplicate key {key:?}")));
            }
            parser.expect(':')?;
            let value = parser.value()?;
            keys.push((key, value));
            match parser.peek() {
                Some(',') => {
                    parser.advance()?;
                }
                Some('}') => {
                    parser.advance()?;
                    break;
                }
                _ => {
                    return Err(parser.error("expected ',' or '}' in INFO object"));
                }
            }
        }
    }
    parser.finish("INFO JSON")?;
    let mut info = Info {
        format_version: 0,
        generator: String::new(),
        generator_version: String::new(),
        source_digest: String::new(),
        document_id: String::new(),
        title: None,
        lang: None,
        chunk_count: 0,
        style_count: 0,
        content_count: 0,
        atlas_count: 0,
        image_count: 0,
    };
    for (key, value) in keys {
        match key.as_str() {
            "format_version" => info.format_version = value.into_u32(&key)?,
            "generator" => info.generator = value.into_string(&key)?,
            "generator_version" => info.generator_version = value.into_string(&key)?,
            "source_digest" => info.source_digest = value.into_string(&key)?,
            "document_id" => info.document_id = value.into_string(&key)?,
            "title" => info.title = Some(value.into_string(&key)?),
            "lang" => info.lang = Some(value.into_string(&key)?),
            "chunk_count" => info.chunk_count = value.into_u32(&key)?,
            "style_count" => info.style_count = value.into_u32(&key)?,
            "content_count" => info.content_count = value.into_u32(&key)?,
            "atlas_count" => info.atlas_count = value.into_u32(&key)?,
            "image_count" => info.image_count = value.into_u32(&key)?,
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    format!("INFO: unknown key {other:?}"),
                ));
            }
        }
    }
    Ok(info)
}

/// A JSON value in the INFO subset: a string or an integer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonValue {
    Str(String),
    Num(u64),
}

impl JsonValue {
    fn into_string(self, key: &str) -> Result<String> {
        match self {
            Self::Str(s) => Ok(s),
            Self::Num(_) => Err(Error::new(
                ErrorKind::InvalidValue,
                format!("INFO: key {key:?} must be a string"),
            )),
        }
    }

    fn into_u32(self, key: &str) -> Result<u32> {
        match self {
            Self::Num(n) => u32::try_from(n).map_err(|_| {
                Error::new(
                    ErrorKind::InvalidValue,
                    format!("INFO: key {key:?} value {n} does not fit u32"),
                )
            }),
            Self::Str(_) => Err(Error::new(
                ErrorKind::InvalidValue,
                format!("INFO: key {key:?} must be an integer"),
            )),
        }
    }
}

/// The deterministic JSON subset parser (SPEC.md §2.1).
struct InfoJson<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> InfoJson<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }

    fn error(&self, message: impl Into<String>) -> Error {
        Error::new(
            ErrorKind::InvalidValue,
            format!("INFO JSON: {}", message.into()),
        )
    }

    /// The next character, or `None` at the end of input.
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    /// Consume the next character (advancing by its full UTF-8 length).
    fn advance(&mut self) -> Result<()> {
        let ch = self
            .peek()
            .ok_or_else(|| self.error("expected a character, found end of input"))?;
        self.pos += ch.len_utf8();
        Ok(())
    }

    fn expect(&mut self, ch: char) -> Result<()> {
        match self.peek() {
            Some(actual) if actual == ch => self.advance(),
            Some(actual) => {
                Err(self.error(format!("expected '{ch}' at {}, found '{actual}'", self.pos)))
            }
            None => Err(self.error(format!(
                "expected '{ch}' at {}, found end of input",
                self.pos
            ))),
        }
    }

    fn string(&mut self) -> Result<String> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let ch = self.peek();
            match ch {
                None => return Err(self.error("unterminated string")),
                Some('"') => {
                    self.advance()?;
                    return Ok(out);
                }
                Some('\\') => {
                    self.advance()?;
                    let esc = self.peek();
                    match esc {
                        None => return Err(self.error("unterminated escape")),
                        Some('"') => {
                            out.push('"');
                            self.advance()?;
                        }
                        Some('\\') => {
                            out.push('\\');
                            self.advance()?;
                        }
                        Some('n') => {
                            out.push('\n');
                            self.advance()?;
                        }
                        Some('r') => {
                            out.push('\r');
                            self.advance()?;
                        }
                        Some('t') => {
                            out.push('\t');
                            self.advance()?;
                        }
                        Some('b') => {
                            out.push('\u{0008}');
                            self.advance()?;
                        }
                        Some('f') => {
                            out.push('\u{000c}');
                            self.advance()?;
                        }
                        Some('u') => {
                            self.advance()?;
                            let hex4 = self.text.get(self.pos..self.pos + 4);
                            let code = match hex4 {
                                Some(h) => u32::from_str_radix(h, 16).ok(),
                                None => None,
                            };
                            let code = code.ok_or_else(|| self.error("bad \\u escape"))?;
                            let ch =
                                char::from_u32(code).ok_or_else(|| self.error("bad \\u escape"))?;
                            out.push(ch);
                            self.pos += 4;
                        }
                        Some(other) => {
                            return Err(self.error(format!("bad escape '\\{other}'")));
                        }
                    }
                }
                Some(ch) => {
                    out.push(ch);
                    self.advance()?;
                }
            }
        }
    }

    fn value(&mut self) -> Result<JsonValue> {
        match self.peek() {
            Some('"') => Ok(JsonValue::Str(self.string()?)),
            Some('-') => Err(self.error("negative numbers are not part of the INFO subset")),
            Some(ch) if ch.is_ascii_digit() => {
                let start = self.pos;
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.advance()?;
                }
                let digits = &self.text[start..self.pos];
                let value = digits
                    .parse::<u64>()
                    .map_err(|_| self.error(format!("integer {digits:?} does not fit u64")))?;
                Ok(JsonValue::Num(value))
            }
            Some(other) => Err(self.error(format!("unexpected value start '{other}'"))),
            None => Err(self.error("expected a value, found end of input")),
        }
    }

    /// Assert that the parse consumed the whole input.
    fn finish(&self, what: &str) -> Result<()> {
        if self.pos != self.text.len() {
            return Err(self.error(format!(
                "{what}: {} trailing bytes",
                self.text.len() - self.pos
            )));
        }
        Ok(())
    }
}
