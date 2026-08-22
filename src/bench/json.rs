//! A minimal JSON writer.
//!
//! Results are the deliverable, so they are machine-readable by construction
//! rather than scraped out of human-readable output later. Hand-rolled because
//! the engine holds itself to two dependencies and the harness should not be
//! the reason a reviewer cannot build the repo.

use std::fmt::Write as _;

/// A JSON value, built up in memory and rendered once.
#[derive(Clone, Debug)]
pub enum J {
    Null,
    Bool(bool),
    /// Rendered with the given number of decimal places, so a throughput and a
    /// p-value do not both come out with seventeen digits.
    F(f64, usize),
    I(i64),
    S(String),
    A(Vec<J>),
    O(Vec<(String, J)>),
}

impl J {
    pub fn s(v: impl Into<String>) -> J {
        J::S(v.into())
    }
    pub fn f(v: f64) -> J {
        J::F(v, 4)
    }
    /// A float with an explicit precision, for values where trailing noise
    /// would be read as significance it does not have.
    pub fn fp(v: f64, places: usize) -> J {
        J::F(v, places)
    }
    pub fn i(v: impl Into<i64>) -> J {
        J::I(v.into())
    }
    pub fn u(v: u64) -> J {
        J::I(v as i64)
    }
    pub fn arr(v: Vec<J>) -> J {
        J::A(v)
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::I(v) => {
                let _ = write!(out, "{v}");
            }
            J::F(v, p) => {
                // NaN and infinity are not JSON; emitting them silently
                // produces a file no parser will read, which is worse than
                // recording that the value was undefined.
                if v.is_finite() {
                    let _ = write!(out, "{v:.*}", p);
                } else {
                    out.push_str("null");
                }
            }
            J::S(s) => escape(s, out),
            J::A(items) => {
                out.push('[');
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    it.write(out);
                }
                out.push(']');
            }
            J::O(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    escape(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Build an object without repeating `.to_string()` at every key.
#[macro_export]
macro_rules! jobj {
    ($($k:expr => $v:expr),* $(,)?) => {
        $crate::bench::J::O(vec![ $( ($k.to_string(), $v) ),* ])
    };
}
