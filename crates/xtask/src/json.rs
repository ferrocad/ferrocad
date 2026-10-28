//! A minimal JSON reader, enough for `cargo metadata` and the edition contract.
//!
//! `xtask` has no dependencies on purpose: it is built and run by the very cargo
//! it drives, often in a preflight job where nothing but cargo is installed.
//! Pulling in `serde_json` would put a compiled crate (and its build time) in
//! front of `cargo xtask editions --check`, so this module parses the handful of
//! shapes `cargo metadata` produces instead. It is a full reader for the JSON
//! grammar (objects, arrays, strings with escapes, numbers, `true`/`false`/`null`)
//! but deliberately offers no numbers/`Option` ergonomics beyond what we use.

/// Maximum nesting depth, so a malformed document cannot blow the stack.
const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// Look up an object field (objects keep their declaration order).
    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(fields) => Some(fields),
            _ => None,
        }
    }
}

/// Parse a complete JSON document.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    let value = parser.value(0)?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(parser.error("trailing data"));
    }
    Ok(value)
}

/// Quote `text` as a JSON string (used by the contract writer).
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> String {
        format!("invalid JSON at byte {}: {message}", self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected `{}`", byte as char)))
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(())
        } else {
            Err(self.error(&format!("expected `{word}`")))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting too deep"));
        }
        self.skip_ws();
        match self.peek().ok_or_else(|| self.error("unexpected end of input"))? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => Ok(Value::String(self.string()?)),
            b't' => {
                self.literal("true")?;
                Ok(Value::Bool(true))
            }
            b'f' => {
                self.literal("false")?;
                Ok(Value::Bool(false))
            }
            b'n' => {
                self.literal("null")?;
                Ok(Value::Null)
            }
            b'-' | b'0'..=b'9' => self.number(),
            other => Err(self.error(&format!("unexpected byte 0x{other:02x}"))),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(fields));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected an object key"));
            }
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.value(depth + 1)?;
            fields.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(fields));
                }
                _ => return Err(self.error("expected `,` or `}`")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("expected `,` or `]`")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        self.digits();
        if self.peek() == Some(b'.') {
            self.pos += 1;
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            self.digits();
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.error("number is not ASCII"))?;
        text.parse::<f64>()
            .map(Value::Number)
            .map_err(|_| self.error("invalid number"))
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let byte = self.peek().ok_or_else(|| self.error("unterminated string"))?;
            self.pos += 1;
            match byte {
                b'"' => break,
                b'\\' => {
                    let escape = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let character = self.unicode_escape()?;
                            let mut buffer = [0u8; 4];
                            out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                        }
                        other => {
                            return Err(
                                self.error(&format!("invalid escape `\\{}`", other as char))
                            );
                        }
                    }
                }
                0x00..=0x1f => return Err(self.error("control character in string")),
                byte => out.push(byte),
            }
        }
        String::from_utf8(out).map_err(|_| self.error("string is not valid UTF-8"))
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let high = self.hex4()?;
        let code = if (0xd800..=0xdbff).contains(&high) {
            if self.peek() == Some(b'\\') && self.bytes.get(self.pos + 1) == Some(&b'u') {
                self.pos += 2;
                let low = self.hex4()?;
                if !(0xdc00..=0xdfff).contains(&low) {
                    return Err(self.error("invalid low surrogate"));
                }
                0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
            } else {
                return Err(self.error("lone high surrogate"));
            }
        } else if (0xdc00..=0xdfff).contains(&high) {
            return Err(self.error("lone low surrogate"));
        } else {
            high
        };
        char::from_u32(code).ok_or_else(|| self.error("invalid unicode scalar"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let bytes = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.error("truncated \\u escape"))?;
        let text = std::str::from_utf8(bytes).map_err(|_| self.error("invalid \\u escape"))?;
        let value =
            u32::from_str_radix(text, 16).map_err(|_| self.error("invalid \\u escape"))?;
        self.pos += 4;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, parse, quote};

    #[test]
    fn reads_the_shapes_metadata_uses() {
        let value = parse(
            r#"{"packages":[{"name":"a","kind":null,"optional":true,
               "features":{"default":["dep:b"]},"n":1.5e3}],
               "workspace_members":[]}"#,
        )
        .expect("parse");
        let packages = value.field("packages").and_then(Value::as_array).expect("packages");
        let package = &packages[0];
        assert_eq!(package.field("name").and_then(Value::as_str), Some("a"));
        assert_eq!(package.field("kind"), Some(&Value::Null));
        assert_eq!(package.field("optional").and_then(Value::as_bool), Some(true));
        let features = package.field("features").expect("features");
        assert_eq!(
            features
                .field("default")
                .and_then(Value::as_array)
                .map(|items| items.to_vec()),
            Some(vec![Value::String("dep:b".to_string())])
        );
    }

    #[test]
    fn decodes_escapes_and_quotes_them_again() {
        let value = parse(r#""a\\b\"c\u00e9\ud83d\ude00""#).expect("parse");
        assert_eq!(value.as_str(), Some("a\\b\"cé😀"));
        let quoted = quote("a\\b\"cé😀\n");
        assert_eq!(
            parse(&quoted).expect("reparse").as_str(),
            Some("a\\b\"cé😀\n")
        );
    }

    #[test]
    fn rejects_trailing_data() {
        let error = parse("{} x").expect_err("trailing data must fail");
        assert!(error.contains("trailing data"), "{error}");
    }
}
