//! Minimal, dependency-free JSON. Deterministic output (object key order is
//! insertion order). One parser, one writer, full RFC 8259 coverage.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 => {
                Some(*n as i64)
            }
            _ => None,
        }
    }
    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().filter(|&v| (0..=u32::MAX as i64).contains(&v)).map(|v| v as u32)
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(o) => o.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }
    pub fn u32_field(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(|v| v.as_u32())
    }

    pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    pub fn arr(v: Vec<Json>) -> Json {
        Json::Arr(v)
    }
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }
    pub fn num(v: impl Into<f64>) -> Json {
        Json::Num(v.into())
    }
    pub fn u32n(v: u32) -> Json {
        Json::Num(v as f64)
    }

    /// Compact serialization.
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let mut out = String::with_capacity(256);
        write_json(self, &mut out, None, 0);
        out
    }
    /// Pretty serialization, two-space indent.
    pub fn to_pretty(&self) -> String {
        let mut out = String::with_capacity(512);
        write_json(self, &mut out, Some(2), 0);
        out
    }

    pub fn parse(input: &str) -> Result<Json, ParseError> {
        let mut p = Parser { bytes: input.as_bytes(), pos: 0 };
        p.skip_ws();
        let v = p.value()?;
        p.skip_ws();
        if p.pos != p.bytes.len() {
            return Err(p.err("trailing characters after JSON value"));
        }
        Ok(v)
    }
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub offset: usize,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON parse error at line {} col {}: {}", self.line, self.col, self.message)
    }
}
impl std::error::Error for ParseError {}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: &str) -> ParseError {
        let mut line = 1usize;
        let mut col = 1usize;
        for &b in &self.bytes[..self.pos.min(self.bytes.len())] {
            if b == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        ParseError { offset: self.pos, line, col, message: msg.to_string() }
    }

    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn expect(&mut self, b: u8) -> Result<(), ParseError> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected '{}'", b as char)))
        }
    }

    fn value(&mut self) -> Result<Json, ParseError> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected character")),
            None => Err(self.err("unexpected end of input")),
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Result<Json, ParseError> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(v)
        } else {
            Err(self.err(&format!("invalid literal, expected '{word}'")))
        }
    }

    fn object(&mut self) -> Result<Json, ParseError> {
        self.expect(b'{')?;
        let mut pairs: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(pairs));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected string key"));
            }
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let val = self.value()?;
            pairs.push((key, val));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(pairs));
                }
                _ => return Err(self.err("expected ',' or '}' in object")),
            }
        }
    }

    fn array(&mut self) -> Result<Json, ParseError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let b = self.peek().ok_or_else(|| self.err("unterminated string"))?;
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => return Err(self.err("invalid escape")),
                    }
                }
                0x00..=0x1F => return Err(self.err("control character in string")),
                _ => {
                    let start = self.pos;
                    while let Some(&n) = self.bytes.get(self.pos) {
                        if n == b'"' || n == b'\\' || n < 0x20 {
                            break;
                        }
                        self.pos += 1;
                    }
                    let chunk = std::str::from_utf8(&self.bytes[start..self.pos])
                        .map_err(|_| self.err("invalid UTF-8 in string"))?;
                    out.push_str(chunk);
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, ParseError> {
        let hex4 = |p: &mut Self| -> Result<u16, ParseError> {
            let mut v: u16 = 0;
            for _ in 0..4 {
                let b = p.peek().ok_or_else(|| p.err("truncated \\u escape"))?;
                p.pos += 1;
                let d = match b {
                    b'0'..=b'9' => b - b'0',
                    b'a'..=b'f' => b - b'a' + 10,
                    b'A'..=b'F' => b - b'A' + 10,
                    _ => return Err(p.err("invalid hex digit in \\u escape")),
                };
                v = (v << 4) | d as u16;
            }
            Ok(v)
        };
        let u = hex4(self)?;
        if (0xD800..0xDC00).contains(&u) {
            // high surrogate: must be followed by \uDC00..\uDFFF
            if self.peek() == Some(b'\\') {
                self.pos += 1;
                let next = self.peek().ok_or_else(|| self.err("truncated surrogate pair"))?;
                if next != b'u' {
                    return Err(self.err("unpaired surrogate in escape"));
                }
                self.pos += 1;
                let lo = hex4(self)?;
                if !(0xDC00..0xE000).contains(&lo) {
                    return Err(self.err("invalid low surrogate"));
                }
                let c = 0x10000u32 + ((u as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                char::from_u32(c).ok_or_else(|| self.err("invalid code point"))
            } else {
                Err(self.err("unpaired surrogate in escape"))
            }
        } else if (0xDC00..0xE000).contains(&u) {
            Err(self.err("unpaired low surrogate in escape"))
        } else {
            char::from_u32(u as u32).ok_or_else(|| self.err("invalid code point"))
        }
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        // int part
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if let Some(b'0'..=b'9') = self.peek() {
                    return Err(self.err("leading zeros are not allowed"));
                }
            }
            Some(b'1'..=b'9') => {
                while let Some(b'0'..=b'9') = self.peek() {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err("invalid number")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("digit expected after '.'"));
            }
            while let Some(b'0'..=b'9') = self.peek() {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("digit expected in exponent"));
            }
            while let Some(b'0'..=b'9') = self.peek() {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| self.err("number out of range"))
    }
}

fn write_json(v: &Json, out: &mut String, indent: Option<usize>, depth: usize) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Num(n) => write_number(*n, out),
        Json::Str(s) => write_string(s, out),
        Json::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if let Some(w) = indent {
                    out.push('\n');
                    out.extend(std::iter::repeat_n(' ', w * (depth + 1)));
                }
                write_json(item, out, indent, depth + 1);
            }
            if let Some(w) = indent {
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', w * depth));
            }
            out.push(']');
        }
        Json::Obj(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, val)) in pairs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if let Some(w) = indent {
                    out.push('\n');
                    out.extend(std::iter::repeat_n(' ', w * (depth + 1)));
                }
                write_string(k, out);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_json(val, out, indent, depth + 1);
            }
            if let Some(w) = indent {
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', w * depth));
            }
            out.push('}');
        }
    }
}

fn write_number(n: f64, out: &mut String) {
    if n.is_nan() || n.is_infinite() {
        // JSON has no NaN/Infinity; emit as string-ish null marker so output stays valid JSON.
        out.push_str("null");
        return;
    }
    if n == n.trunc() && n.abs() < 9.007_199_254_740_992e15 {
        // integral and safely representable: emit without decimal point
        if n == 0.0 && n.is_sign_negative() {
            out.push_str("-0");
        } else {
            out.push_str(&itoa(n.trunc() as i128));
        }
    } else {
        out.push_str(&rust_f64_to_string(n));
    }
}

fn itoa(mut v: i128) -> String {
    if v == 0 {
        return "0".to_string();
    }
    let neg = v < 0;
    if neg {
        v = -v;
    }
    let mut buf = [0u8; 44];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    String::from_utf8_lossy(&buf[i..]).into_owned()
}

/// Shortest round-trip float formatting without pulling in ryu.
fn rust_f64_to_string(n: f64) -> String {
    let mut prec = 1usize;
    loop {
        let s = format!("{:.*e}", prec.saturating_sub(1), n);
        if s.parse::<f64>() == Ok(n) {
            let (mant, exp) = s.split_once('e').unwrap();
            let exp: i32 = exp.parse().unwrap();
            if (-4..16).contains(&exp) {
                return format!("{}", n);
            }
            // normalize exponent notation like "1.5e3" -> "1.5e3"
            let mant = mant.trim_end_matches('0').trim_end_matches('.');
            return format!("{}e{}", mant, exp);
        }
        prec += 1;
        if prec > 17 {
            return format!("{}", n);
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                let hex = format!("{:04x}", c as u32);
                out.push_str(&hex);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_objects() {
        let src = r#"{"a":1,"b":[true,false,null,"x\ny"],"c":{"d":-2.5e3,"e":"éA"}}"#;
        let v = Json::parse(src).unwrap();
        assert_eq!(v.get("a").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(v.str_field("b"), None);
        let out = v.to_string();
        let v2 = Json::parse(&out).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "", "{", "[1,]", "{\"a\":}", "01", "\"\\x\"", "\"unterminated", "{\"a\" 1}",
            "[1 2]", "\"\\ud800\"", "1e", "nan", "{'a':1}",
        ] {
            assert!(Json::parse(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn number_formats() {
        assert_eq!(Json::Num(1.0).to_string(), "1");
        assert_eq!(Json::Num(-0.0).to_string(), "-0");
        assert_eq!(Json::Num(2.5).to_string(), "2.5");
        assert_eq!(Json::Num(1e21).to_string(), "1e21");
        assert_eq!(Json::Num(f64::NAN).to_string(), "null");
        assert_eq!(Json::parse("3.0").unwrap(), Json::Num(3.0));
        assert_eq!(Json::parse("1.5E+2").unwrap().as_f64(), Some(150.0));
    }

    #[test]
    fn surrogate_pairs() {
        let v = Json::parse("\"\\ud83d\\ude00\"").unwrap();
        assert_eq!(v.as_str(), Some("\u{1F600}"));
    }

    #[test]
    fn insertion_order_preserved() {
        let v = Json::parse(r#"{"z":1,"a":2,"m":3}"#).unwrap();
        let keys: Vec<&str> = v.as_obj().unwrap().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["z", "a", "m"]);
        assert!(v.to_string().starts_with(r#"{"z":1,"a":2,"m":3}"#));
    }

    #[test]
    fn pretty_stable() {
        let v = Json::obj(vec![
            ("n", Json::num(1)),
            ("list", Json::arr(vec![Json::str("x"), Json::Null])),
        ]);
        let p = v.to_pretty();
        assert!(p.contains("  \"n\": 1,\n"));
        assert!(Json::parse(&p).unwrap() == v);
    }

    #[test]
    fn deep_nesting_ok() {
        let depth = 256;
        let src = "[".repeat(depth) + &"]".repeat(depth);
        let v = Json::parse(&src).unwrap();
        assert_eq!(v.to_string(), src);
    }
}
