// Built-in functions. Dispatched by uppercased name.

use crate::value::Value;

/// Is `name` a pure builtin handled by `apply`?
pub fn is_builtin(name: &str) -> bool {
    BUILTINS.iter().any(|(n, _)| *n == name)
}

/// Apply a *pure* builtin by name. `args` are already-evaluated values.
pub fn apply(name: &str, args: Vec<Value>) -> Result<Value, String> {
    for (bn, f) in BUILTINS {
        if *bn == name {
            return f(args);
        }
    }
    Err(format!("?SYNTAX ERROR: unknown function {}", name))
}

type FnSig = fn(Vec<Value>) -> Result<Value, String>;

// Table of (name, function).
static BUILTINS: &[(&str, FnSig)] = &[
    ("RND", rnd),
    ("INT", int_fn),
    ("ABS", abs_fn),
    ("SGN", sgn_fn),
    ("SQR", sqr_fn),
    ("SIN", sin_fn),
    ("COS", cos_fn),
    ("TAN", tan_fn),
    ("ATN", atn_fn),
    ("EXP", exp_fn),
    ("LOG", log_fn),
    ("LEN", len_fn),
    ("MID$", mid_fn),
    ("LEFT$", left_fn),
    ("RIGHT$", right_fn),
    ("CHR$", chr_fn),
    ("STR$", str_fn),
    ("VAL", val_fn),
    ("ASC", asc_fn),
];

fn num1(args: &[Value]) -> Result<f64, String> {
    if args.len() != 1 {
        return Err("?SYNTAX ERROR: wrong number of arguments".into());
    }
    args[0].as_num()
}
fn str1(args: &[Value]) -> Result<&str, String> {
    if args.len() != 1 {
        return Err("?SYNTAX ERROR: wrong number of arguments".into());
    }
    match &args[0] {
        Value::Str(s) => Ok(s.as_str()),
        _ => Err("?TYPE MISMATCH ERROR".into()),
    }
}

fn rnd(args: Vec<Value>) -> Result<Value, String> {
    let x = num1(&args)?;
    // Applesoft: RND(x>0) -> new random in [0,1); RND(0) -> last; RND(x<0) seeds.
    // Simplified: always return a fresh [0,1) for x > 0; reuse last otherwise.
    if x > 0.0 {
        Ok(Value::Num(thread_rng_f64()))
    } else if x < 0.0 {
        // seed ignored; just produce a value
        Ok(Value::Num(thread_rng_f64()))
    } else {
        Ok(Value::Num(thread_rng_f64()))
    }
}
fn int_fn(a: Vec<Value>) -> Result<Value, String> {
    let n = num1(&a)?;
    Ok(Value::Num(n.floor()))
}
fn abs_fn(a: Vec<Value>) -> Result<Value, String> {
    let n = num1(&a)?;
    Ok(Value::Num(n.abs()))
}
fn sgn_fn(a: Vec<Value>) -> Result<Value, String> {
    let n = num1(&a)?;
    Ok(Value::Num(if n > 0.0 {
        1.0
    } else if n < 0.0 {
        -1.0
    } else {
        0.0
    }))
}
fn sqr_fn(a: Vec<Value>) -> Result<Value, String> {
    let n = num1(&a)?;
    if n < 0.0 {
        return Err("?ILLEGAL QUANTITY ERROR".into());
    }
    Ok(Value::Num(n.sqrt()))
}
fn sin_fn(a: Vec<Value>) -> Result<Value, String> {
    Ok(Value::Num(num1(&a)?.sin()))
}
fn cos_fn(a: Vec<Value>) -> Result<Value, String> {
    Ok(Value::Num(num1(&a)?.cos()))
}
fn tan_fn(a: Vec<Value>) -> Result<Value, String> {
    Ok(Value::Num(num1(&a)?.tan()))
}
fn atn_fn(a: Vec<Value>) -> Result<Value, String> {
    Ok(Value::Num(num1(&a)?.atan()))
}
fn exp_fn(a: Vec<Value>) -> Result<Value, String> {
    Ok(Value::Num(num1(&a)?.exp()))
}
fn log_fn(a: Vec<Value>) -> Result<Value, String> {
    let n = num1(&a)?;
    if n <= 0.0 {
        return Err("?ILLEGAL QUANTITY ERROR".into());
    }
    Ok(Value::Num(n.ln()))
}
fn len_fn(a: Vec<Value>) -> Result<Value, String> {
    let s = str1(&a)?;
    Ok(Value::Num(s.len() as f64))
}
fn mid_fn(a: Vec<Value>) -> Result<Value, String> {
    if a.len() != 2 && a.len() != 3 {
        return Err("?SYNTAX ERROR: MID$ needs 2 or 3 args".into());
    }
    let s = match &a[0] {
        Value::Str(s) => s.clone(),
        _ => return Err("?TYPE MISMATCH ERROR".into()),
    };
    let start = a[1].as_num()? as usize;
    if start < 1 {
        return Err("?ILLEGAL QUANTITY ERROR".into());
    }
    let len = if a.len() == 3 {
        a[2].as_num()? as usize
    } else {
        s.len()
    };
    let start_idx = start - 1;
    if start_idx >= s.len() {
        return Ok(Value::Str(String::new()));
    }
    let end = (start_idx + len).min(s.len());
    Ok(Value::Str(s[start_idx..end].to_string()))
}
fn left_fn(a: Vec<Value>) -> Result<Value, String> {
    let s = str1(&a[0..1])?.to_string();
    let n = a[1].as_num()? as usize;
    Ok(Value::Str(s.chars().take(n).collect()))
}
fn right_fn(a: Vec<Value>) -> Result<Value, String> {
    let s = str1(&a[0..1])?.to_string();
    let n = a[1].as_num()? as usize;
    let chars: Vec<char> = s.chars().collect();
    let take = n.min(chars.len());
    let start = chars.len() - take;
    Ok(Value::Str(chars[start..].iter().collect()))
}
fn chr_fn(a: Vec<Value>) -> Result<Value, String> {
    let n = num1(&a)? as u32;
    if n > 0x10FFFF {
        return Err("?ILLEGAL QUANTITY ERROR".into());
    }
    match char::from_u32(n) {
        Some(c) => Ok(Value::Str(c.to_string())),
        None => Err("?ILLEGAL QUANTITY ERROR".into()),
    }
}
fn str_fn(a: Vec<Value>) -> Result<Value, String> {
    let n = num1(&a)?;
    Ok(Value::Str(crate::value::format_basic_number(n)))
}
fn val_fn(a: Vec<Value>) -> Result<Value, String> {
    let s = str1(&a)?;
    Ok(Value::Num(crate::value::parse_leading_num_pub(s)))
}
fn asc_fn(a: Vec<Value>) -> Result<Value, String> {
    let s = str1(&a)?;
    if s.is_empty() {
        return Err("?ILLEGAL QUANTITY ERROR".into());
    }
    Ok(Value::Num(s.as_bytes()[0] as f64))
}

// Simple thread-local RNG so we don't pull in an external crate.
fn thread_rng_f64() -> f64 {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = Cell::new(seed());
    }
    STATE.with(|c| {
        let mut s = c.get();
        if s == 0 {
            s = seed();
        }
        // xorshift64
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        c.set(s);
        // map to [0,1)
        (s >> 11) as f64 / (1u64 << 53) as f64
    })
}

fn seed() -> u64 {
    // deterministic-ish seed from system time via std (allowed in lib code).
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64 | 0x9E3779B97F4A7C15)
        .unwrap_or(0x12345678)
}
