//! Reading results back.
//!
//! `results/` is the source of truth: plots and claim verification both read
//! committed files rather than re-running the engine. That is what makes the
//! proof auditable -- a reviewer can check that a published figure follows
//! from the recorded measurements without trusting the code that made it.
//!
//! A minimal parser rather than a dependency, for the same reason the writer
//! is hand-rolled: the harness must never be the reason the repository will
//! not build.

use super::J;

#[derive(Debug)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "json: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

pub fn parse(s: &str) -> Result<J, ParseError> {
    let b = s.as_bytes();
    let mut p = Parser { b, i: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != b.len() {
        return Err(ParseError(format!("trailing input at byte {}", p.i)));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), ParseError> {
        if self.i < self.b.len() && self.b[self.i] == c {
            self.i += 1;
            Ok(())
        } else {
            Err(ParseError(format!("expected '{}' at byte {}", c as char, self.i)))
        }
    }

    fn lit(&mut self, s: &str) -> bool {
        if self.b[self.i..].starts_with(s.as_bytes()) {
            self.i += s.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<J, ParseError> {
        self.ws();
        if self.i >= self.b.len() {
            return Err(ParseError("unexpected end of input".into()));
        }
        match self.b[self.i] {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Ok(J::S(self.string()?)),
            b't' => {
                if self.lit("true") { Ok(J::Bool(true)) } else { Err(self.bad()) }
            }
            b'f' => {
                if self.lit("false") { Ok(J::Bool(false)) } else { Err(self.bad()) }
            }
            b'n' => {
                if self.lit("null") { Ok(J::Null) } else { Err(self.bad()) }
            }
            _ => self.number(),
        }
    }

    fn bad(&self) -> ParseError {
        ParseError(format!("unexpected token at byte {}", self.i))
    }

    fn object(&mut self) -> Result<J, ParseError> {
        self.eat(b'{')?;
        let mut out = Vec::new();
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == b'}' {
            self.i += 1;
            return Ok(J::O(out));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(b':')?;
            let v = self.value()?;
            out.push((k, v));
            self.ws();
            if self.i < self.b.len() && self.b[self.i] == b',' {
                self.i += 1;
                continue;
            }
            self.eat(b'}')?;
            return Ok(J::O(out));
        }
    }

    fn array(&mut self) -> Result<J, ParseError> {
        self.eat(b'[')?;
        let mut out = Vec::new();
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == b']' {
            self.i += 1;
            return Ok(J::A(out));
        }
        loop {
            out.push(self.value()?);
            self.ws();
            if self.i < self.b.len() && self.b[self.i] == b',' {
                self.i += 1;
                continue;
            }
            self.eat(b']')?;
            return Ok(J::A(out));
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.eat(b'"')?;
        let mut out = String::new();
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let c = *self.b.get(self.i).ok_or_else(|| ParseError("escape at eof".into()))?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let hex = std::str::from_utf8(&self.b[self.i..self.i + 4])
                                .map_err(|_| ParseError("bad \\u".into()))?;
                            let n = u32::from_str_radix(hex, 16)
                                .map_err(|_| ParseError("bad \\u".into()))?;
                            self.i += 4;
                            out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
                        }
                        other => return Err(ParseError(format!("bad escape \\{}", other as char))),
                    }
                }
                _ => {
                    // Copy the whole UTF-8 sequence, not one byte.
                    let start = self.i;
                    self.i += 1;
                    while self.i < self.b.len() && (self.b[self.i] & 0xC0) == 0x80 {
                        self.i += 1;
                    }
                    out.push_str(
                        std::str::from_utf8(&self.b[start..self.i])
                            .map_err(|_| ParseError("bad utf8".into()))?,
                    );
                }
            }
        }
        Err(ParseError("unterminated string".into()))
    }

    fn number(&mut self) -> Result<J, ParseError> {
        let start = self.i;
        if self.i < self.b.len() && (self.b[self.i] == b'-' || self.b[self.i] == b'+') {
            self.i += 1;
        }
        while self.i < self.b.len()
            && matches!(self.b[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'-' | b'+')
        {
            self.i += 1;
        }
        let txt = std::str::from_utf8(&self.b[start..self.i])
            .map_err(|_| ParseError("bad number".into()))?;
        if txt.is_empty() {
            return Err(ParseError(format!("expected a value at byte {start}")));
        }
        // Integers stay integers so a key count does not come back as 1.0e6.
        if !txt.contains(['.', 'e', 'E']) {
            if let Ok(v) = txt.parse::<i64>() {
                return Ok(J::I(v));
            }
        }
        txt.parse::<f64>().map(|v| J::F(v, 6)).map_err(|_| ParseError(format!("bad number {txt}")))
    }
}

/// Navigation helpers. Reading a result should not require pattern matching at
/// every level.
impl J {
    pub fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::O(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a dotted path, e.g. `series.throughput.ops_per_s`.
    pub fn path(&self, path: &str) -> Option<&J> {
        let mut cur = self;
        for part in path.split('.') {
            cur = if let Ok(i) = part.parse::<usize>() {
                cur.at(i)?
            } else {
                cur.get(part)?
            };
        }
        Some(cur)
    }

    pub fn at(&self, i: usize) -> Option<&J> {
        match self {
            J::A(items) => items.get(i),
            _ => None,
        }
    }

    pub fn items(&self) -> &[J] {
        match self {
            J::A(items) => items,
            _ => &[],
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            J::F(v, _) => Some(*v),
            J::I(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            J::I(v) if *v >= 0 => Some(*v as u64),
            J::F(v, _) if *v >= 0.0 => Some(*v as u64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::S(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            J::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Numeric value at a dotted path.
    pub fn num(&self, path: &str) -> Option<f64> {
        self.path(path)?.as_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobj;

    #[test]
    fn round_trips_through_render_and_parse() {
        let v = jobj! {
            "a" => J::u(42),
            "b" => J::fp(1.5, 3),
            "c" => J::s("hi \"there\"\n"),
            "d" => J::arr(vec![J::u(1), J::Bool(false), J::Null]),
            "e" => jobj! { "nested" => J::fp(-0.25, 4) },
        };
        let back = parse(&v.render()).expect("parse");
        assert_eq!(back.num("a"), Some(42.0));
        assert_eq!(back.num("b"), Some(1.5));
        assert_eq!(back.path("c").unwrap().as_str(), Some("hi \"there\"\n"));
        assert_eq!(back.path("d.1").unwrap().as_bool(), Some(false));
        assert_eq!(back.num("e.nested"), Some(-0.25));
    }

    #[test]
    fn integers_do_not_become_floats() {
        let back = parse("{\"keys\":10000000}").unwrap();
        assert!(matches!(back.get("keys"), Some(J::I(10_000_000))));
        assert_eq!(back.get("keys").unwrap().as_u64(), Some(10_000_000));
    }

    #[test]
    fn missing_paths_are_none_not_panics() {
        let v = parse("{\"a\":{\"b\":1}}").unwrap();
        assert_eq!(v.num("a.b"), Some(1.0));
        assert_eq!(v.num("a.z"), None);
        assert_eq!(v.num("nope.deep.path"), None);
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in ["{", "{\"a\":}", "[1,2", "\"unterminated", "{\"a\":1}x", ""] {
            assert!(parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn handles_unicode_and_escapes() {
        let v = parse(r#"{"k":"café — ok"}"#).unwrap();
        assert_eq!(v.path("k").unwrap().as_str(), Some("café — ok"));
        let v2 = parse("{\"k\":\"日本語\"}").unwrap();
        assert_eq!(v2.path("k").unwrap().as_str(), Some("日本語"));
    }

    #[test]
    fn arrays_are_indexable_by_path() {
        let v = parse(r#"{"s":[{"t":1},{"t":2}]}"#).unwrap();
        assert_eq!(v.num("s.1.t"), Some(2.0));
        assert_eq!(v.path("s").unwrap().items().len(), 2);
    }
}
