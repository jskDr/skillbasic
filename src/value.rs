// Runtime values.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f64),
    Str(String),
}

impl Value {
    pub fn is_string(&self) -> bool {
        matches!(self, Value::Str(_))
    }

    pub fn as_num(&self) -> Result<f64, String> {
        match self {
            Value::Num(n) => Ok(*n),
            Value::Str(s) => {
                // Applesoft coerces leading numeric string to a number; we accept
                // a clean numeric parse, else 0 (with a leading number fallback).
                parse_leading_num(s)
            }
        }
    }

    pub fn as_str(&self) -> Result<String, String> {
        match self {
            Value::Str(s) => Ok(s.clone()),
            Value::Num(n) => Ok(format_basic_number(*n)),
        }
    }
}

/// Parse a leading numeric prefix of a string (Applesoft VAL semantics);
/// trailing non-numeric chars are ignored. Returns 0 if no leading number.
fn parse_leading_num(s: &str) -> Result<f64, String> {
    let s = s.trim_start();
    let mut end = 0;
    let bytes = s.as_bytes();
    while end < bytes.len()
        && (bytes[end].is_ascii_digit()
            || bytes[end] == b'.'
            || bytes[end] == b'+'
            || bytes[end] == b'-'
            || bytes[end] == b'E'
            || bytes[end] == b'e')
    {
        end += 1;
    }
    if end == 0 {
        return Ok(0.0);
    }
    let head = &s[..end];
    head.parse::<f64>().or(Ok(0.0))
}

/// Public wrapper used by the VAL builtin.
pub fn parse_leading_num_pub(s: &str) -> f64 {
    parse_leading_num(s).unwrap_or(0.0)
}

/// Format a number the way Applesoft PRINT does (simplified):
/// integers without a decimal point, negatives with a leading minus,
/// otherwise a trimmed float.
pub fn format_basic_number(n: f64) -> String {
    if n == 0.0 {
        return "0".to_string();
    } // covers -0
    if n.is_nan() {
        return " NAN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            " INF".to_string()
        } else {
            "-INF".to_string()
        };
    }
    if n.fract() == 0.0 && n.abs() < 1e16 {
        let mut s = String::new();
        let _ = write!(s, "{}", n as i64);
        return s;
    }
    // General float; trim trailing zeros.
    let s = format!("{}", n);
    s
}
