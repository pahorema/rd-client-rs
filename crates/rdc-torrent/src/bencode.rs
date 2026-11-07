//! Bounded bencode decoding with borrowed strings and exact source spans.
use std::{collections::BTreeMap, ops::Range};
use thiserror::Error;

/// A decoded value and its byte range in the input, including delimiters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node<'src> {
    /// Decoded contents.
    pub value: Value<'src>,
    /// Exact encoded range; constructed values may use an empty range.
    pub span: Range<usize>,
}

/// Bencode's four value kinds. Dictionary keys are ordered by raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value<'src> {
    /// A signed integer.
    Integer(i64),
    /// An arbitrary byte string, not necessarily UTF-8.
    Bytes(&'src [u8]),
    /// An ordered sequence.
    List(Vec<Node<'src>>),
    /// A byte-keyed dictionary.
    Dictionary(BTreeMap<&'src [u8], Node<'src>>),
}

/// Resource bounds for untrusted bencode inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum input length, also bounding individual strings.
    pub max_bytes: usize,
    /// Maximum nesting below the root.
    pub max_depth: usize,
    /// Maximum number of values and dictionary keys.
    pub max_nodes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 32 * 1024 * 1024,
            max_depth: 64,
            max_nodes: 1_000_000,
        }
    }
}

/// Malformed bencode or an exceeded resource bound, at a zero-based byte offset.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("bencode error at byte {offset}: {message}")]
pub struct DecodeError {
    /// Byte position where decoding failed.
    pub offset: usize,
    /// Specific malformed token or resource bound.
    pub message: &'static str,
}

/// Decodes exactly one value using the default resource limits.
pub fn decode(input: &[u8]) -> Result<Node<'_>, DecodeError> {
    decode_with_limits(input, Limits::default())
}

/// Decodes exactly one value, rejecting trailing bytes and duplicate keys.
/// Unsorted dictionaries are accepted so their exact raw info hashes remain usable.
pub fn decode_with_limits(input: &[u8], limits: Limits) -> Result<Node<'_>, DecodeError> {
    let mut parser = Parser {
        input,
        pos: 0,
        nodes: 0,
        limits,
    };
    if input.len() > limits.max_bytes {
        return Err(parser.error("input size limit exceeded"));
    }
    // A hard ceiling also bounds stack usage when callers configure larger limits.
    parser.limits.max_depth = limits.max_depth.min(128);
    let node = parser.node(0)?;
    if parser.pos != input.len() {
        return Err(parser.error("trailing bytes"));
    }
    Ok(node)
}

struct Parser<'src> {
    input: &'src [u8],
    pos: usize,
    nodes: usize,
    limits: Limits,
}
impl<'src> Parser<'src> {
    fn error(&self, message: &'static str) -> DecodeError {
        DecodeError {
            offset: self.pos,
            message,
        }
    }
    fn count(&mut self) -> Result<(), DecodeError> {
        if self.nodes >= self.limits.max_nodes {
            return Err(self.error("node limit exceeded"));
        }
        self.nodes += 1;
        Ok(())
    }
    fn node(&mut self, depth: usize) -> Result<Node<'src>, DecodeError> {
        if depth > self.limits.max_depth {
            return Err(self.error("depth limit exceeded"));
        }
        self.count()?;
        let start = self.pos;
        let value = match self.input.get(self.pos) {
            Some(b'i') => {
                self.pos += 1;
                let start = self.pos;
                while self.input.get(self.pos).is_some_and(|c| *c != b'e') {
                    self.pos += 1;
                }
                let token = &self.input[start..self.pos];
                if self.pos == self.input.len() {
                    return Err(self.error("unterminated integer"));
                }
                let digits = token.strip_prefix(b"-").unwrap_or(token);
                if digits.is_empty()
                    || !digits.iter().all(u8::is_ascii_digit)
                    || (digits.len() > 1 && digits[0] == b'0')
                    || token == b"-0"
                {
                    return Err(self.error("invalid integer"));
                }
                let number = std::str::from_utf8(token)
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| self.error("integer out of range"))?;
                self.pos += 1;
                Value::Integer(number)
            }
            Some(b'0'..=b'9') => Value::Bytes(self.bytes()?),
            Some(b'l' | b'd') => {
                let dictionary = self.input[self.pos] == b'd';
                self.pos += 1;
                let mut list = Vec::new();
                let mut map = BTreeMap::new();
                while self.input.get(self.pos) != Some(&b'e') {
                    if self.pos == self.input.len() {
                        return Err(self.error("unterminated container"));
                    }
                    if dictionary {
                        self.count()?;
                        let key = self.bytes()?;
                        if map.contains_key(key) {
                            return Err(self.error("duplicate dictionary key"));
                        }
                        map.insert(key, self.node(depth + 1)?);
                    } else {
                        list.push(self.node(depth + 1)?);
                    }
                }
                self.pos += 1;
                if dictionary {
                    Value::Dictionary(map)
                } else {
                    Value::List(list)
                }
            }
            Some(_) => return Err(self.error("unexpected token")),
            None => return Err(self.error("unexpected end of input")),
        };
        Ok(Node {
            value,
            span: start..self.pos,
        })
    }
    fn bytes(&mut self) -> Result<&'src [u8], DecodeError> {
        let start = self.pos;
        let mut length = 0usize;
        while let Some(digit @ b'0'..=b'9') = self.input.get(self.pos) {
            length = length
                .checked_mul(10)
                .and_then(|n| n.checked_add(usize::from(digit - b'0')))
                .ok_or_else(|| self.error("string length out of range"))?;
            self.pos += 1;
        }
        if self.pos == start
            || self.input.get(self.pos) != Some(&b':')
            || (self.pos - start > 1 && self.input[start] == b'0')
        {
            return Err(self.error("invalid byte string length"));
        }
        self.pos += 1;
        let end = self
            .pos
            .checked_add(length)
            .filter(|&n| n <= self.input.len())
            .ok_or_else(|| self.error("truncated byte string"))?;
        let bytes = &self.input[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
}

/// Encodes canonical bencode, sorting dictionary keys by their bytes.
#[must_use]
pub fn encode(node: &Node<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(node, None, &mut out);
    out
}

/// Encodes `node`, copying decoded values byte for byte from `source`, the input they were
/// decoded from.
///
/// A node with a non-empty span is written as `source[span]`, so unsorted dictionaries and
/// noncanonical encodings survive. A node with an empty span (built with [`Node::new`], or a
/// decoded container whose span was cleared after its contents changed) is encoded
/// canonically, its children following the same rule.
#[must_use]
pub fn encode_with_source(node: &Node<'_>, source: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(source.len());
    encode_into(node, Some(source), &mut out);
    out
}
fn encode_into(node: &Node<'_>, source: Option<&[u8]>, out: &mut Vec<u8>) {
    if let Some(raw) = source
        .filter(|_| !node.span.is_empty())
        .and_then(|source| source.get(node.span.clone()))
    {
        out.extend_from_slice(raw);
        return;
    }
    match &node.value {
        Value::Integer(n) => {
            out.push(b'i');
            out.extend_from_slice(n.to_string().as_bytes());
            out.push(b'e');
        }
        Value::Bytes(s) => encode_bytes(s, out),
        Value::List(items) => {
            out.push(b'l');
            for item in items {
                encode_into(item, source, out);
            }
            out.push(b'e');
        }
        Value::Dictionary(items) => {
            out.push(b'd');
            for (key, item) in items {
                encode_bytes(key, out);
                encode_into(item, source, out);
            }
            out.push(b'e');
        }
    }
}
fn encode_bytes(bytes: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(bytes.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(bytes);
}

impl<'src> Node<'src> {
    /// Returns byte-string contents when this node is a string.
    pub fn bytes(&self) -> Option<&'src [u8]> {
        if let Value::Bytes(s) = self.value {
            Some(s)
        } else {
            None
        }
    }
    /// Returns list contents when this node is a list.
    pub fn list(&self) -> Option<&[Self]> {
        if let Value::List(s) = &self.value {
            Some(s)
        } else {
            None
        }
    }
    /// Looks up a dictionary entry by its raw key.
    pub fn get(&self, key: &[u8]) -> Option<&Self> {
        if let Value::Dictionary(s) = &self.value {
            s.get(key)
        } else {
            None
        }
    }
    /// Constructs a node without a source span.
    pub fn new(value: Value<'src>) -> Self {
        Self { value, span: 0..0 }
    }
}
