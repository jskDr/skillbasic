// Lexer: source text -> Vec<SpanTok>.

use crate::token::{Kw, SpanTok, Tok};

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

#[derive(Debug, Clone)]
pub struct LexError {
    pub msg: String,
    pub pos: usize,
}

pub fn tokenize(src: &str) -> Result<Vec<SpanTok>, LexError> {
    let mut lx = Lexer {
        src: src.as_bytes(),
        pos: 0,
    };
    lx.run()
}

impl<'a> Lexer<'a> {
    fn run(&mut self) -> Result<Vec<SpanTok>, LexError> {
        let mut out = Vec::new();
        loop {
            self.skip_ws_and_remarks_aware()?;
            if self.eof() {
                out.push(SpanTok {
                    tok: Tok::Eof,
                    pos: self.pos,
                });
                return Ok(out);
            }
            let pos = self.pos;
            let b = self.peek();
            match b {
                b'"' => out.push(SpanTok {
                    tok: Tok::String(self.string()?),
                    pos,
                }),
                b'0'..=b'9' | b'.' => out.push(SpanTok {
                    tok: Tok::Number(self.number()?),
                    pos,
                }),
                b'A'..=b'Z' | b'a'..=b'z' => {
                    let (word, is_string) = self.ident();
                    // A trailing `$` marks a string identifier / function name
                    // (e.g. `PROMPT$`, `MID$`). In that case the `$`-suffixed
                    // token is never a keyword, even when the bare word would
                    // match one — this is what lets `PROMPT$(...)` coexist with
                    // the `PROMPT` statement and lets `MID$` be a function
                    // rather than colliding with any future `MID` keyword.
                    if !is_string && let Some(kw) = Kw::from_word(&word) {
                        // REM is special: it consumes the rest of the line
                        // as raw comment text, so comments may contain any
                        // character (apostrophes, underscores, punctuation,
                        // etc.) without triggering a syntax error. We emit
                        // the keyword, then a single String token holding
                        // the verbatim comment text, which `parse_rem`
                        // reads back. This matches real Applesoft, where
                        // REM skips to end-of-line, and fixes the pre-existing
                        // limitation where comments like `REM children's
                        // math quiz` failed with `?SYNTAX ERROR`.
                        if kw == Kw::Rem {
                            out.push(SpanTok {
                                tok: Tok::Keyword(kw),
                                pos,
                            });
                            // Skip the single space between REM and the text
                            // (preserve internal spacing verbatim after it).
                            if !self.eof() && self.peek() == b' ' {
                                self.advance();
                            }
                            let start = self.pos;
                            while !self.eof() {
                                self.advance();
                            }
                            let text = std::str::from_utf8(&self.src[start..self.pos])
                                .unwrap_or("")
                                .trim_end()
                                .to_string();
                            out.push(SpanTok {
                                tok: Tok::String(text),
                                pos: start,
                            });
                            continue;
                        }
                        out.push(SpanTok {
                            tok: Tok::Keyword(kw),
                            pos,
                        });
                        continue;
                    }
                    let name = if is_string {
                        format!("{}$", word)
                    } else {
                        word
                    };
                    out.push(SpanTok {
                        tok: Tok::Ident(name),
                        pos,
                    });
                }
                b'?' => {
                    // ? is alias for PRINT
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Keyword(Kw::Print),
                        pos,
                    });
                }
                b'+' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Plus,
                        pos,
                    });
                }
                b'-' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Minus,
                        pos,
                    });
                }
                b'*' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Star,
                        pos,
                    });
                }
                b'/' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Slash,
                        pos,
                    });
                }
                b'^' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Caret,
                        pos,
                    });
                }
                b'(' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::LParen,
                        pos,
                    });
                }
                b')' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::RParen,
                        pos,
                    });
                }
                b',' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Comma,
                        pos,
                    });
                }
                b';' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Semicolon,
                        pos,
                    });
                }
                b':' => {
                    self.advance();
                    out.push(SpanTok {
                        tok: Tok::Colon,
                        pos,
                    });
                }
                b'=' => {
                    self.advance();
                    out.push(SpanTok { tok: Tok::Eq, pos });
                }
                b'<' => {
                    self.advance();
                    if self.peek() == b'=' {
                        self.advance();
                        out.push(SpanTok { tok: Tok::Le, pos });
                    } else if self.peek() == b'>' {
                        self.advance();
                        out.push(SpanTok { tok: Tok::Ne, pos });
                    } else {
                        out.push(SpanTok { tok: Tok::Lt, pos });
                    }
                }
                b'>' => {
                    self.advance();
                    if self.peek() == b'=' {
                        self.advance();
                        out.push(SpanTok { tok: Tok::Ge, pos });
                    } else {
                        out.push(SpanTok { tok: Tok::Gt, pos });
                    }
                }
                _ => {
                    return Err(LexError {
                        msg: format!("?SYNTAX ERROR: unexpected character {:?}", b as char),
                        pos,
                    });
                }
            }
        }
    }

    fn skip_ws_and_remarks_aware(&mut self) -> Result<(), LexError> {
        loop {
            if self.eof() {
                return Ok(());
            }
            let b = self.peek();
            if b == b' ' || b == b'\t' {
                self.advance();
                continue;
            }
            break;
        }
        Ok(())
    }

    fn string(&mut self) -> Result<String, LexError> {
        let start = self.pos;
        self.advance(); // opening quote
        let mut s = String::new();
        while !self.eof() && self.peek() != b'"' {
            s.push(self.peek() as char);
            self.advance();
        }
        if self.eof() {
            return Err(LexError {
                msg: "?SYNTAX ERROR: unterminated string".into(),
                pos: start,
            });
        }
        self.advance(); // closing quote
        Ok(s)
    }

    fn number(&mut self) -> Result<f64, LexError> {
        let start = self.pos;
        while !self.eof() && self.peek().is_ascii_digit() {
            self.advance();
        }
        if !self.eof() && self.peek() == b'.' {
            self.advance();
            while !self.eof() && self.peek().is_ascii_digit() {
                self.advance();
            }
        }
        // exponent E[+|-]nnn
        if !self.eof() && (self.peek() == b'E' || self.peek() == b'e') {
            self.advance();
            if !self.eof() && (self.peek() == b'+' || self.peek() == b'-') {
                self.advance();
            }
            while !self.eof() && self.peek().is_ascii_digit() {
                self.advance();
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        text.parse::<f64>().map_err(|_| LexError {
            msg: "?SYNTAX ERROR: bad number".into(),
            pos: start,
        })
    }

    /// Read an identifier. Returns (uppercased word, is_string_var).
    fn ident(&mut self) -> (String, bool) {
        let start = self.pos;
        while !self.eof() && self.peek().is_ascii_alphabetic() {
            self.advance();
        }
        // optional digits after the alpha part (e.g. A1)
        while !self.eof() && (self.peek().is_ascii_alphanumeric()) {
            self.advance();
        }
        let mut word: String = std::str::from_utf8(&self.src[start..self.pos])
            .unwrap()
            .to_ascii_uppercase();
        // trailing $ for string variable
        let mut is_string = false;
        if !self.eof() && self.peek() == b'$' {
            self.advance();
            is_string = true;
            // do not include $ in keyword lookup
            // strip trailing $ from word for kw check, but keep alnum as-is
        } else {
            // word stays as uppercased
        }
        // For keyword check, the $ branch already set is_string; word may still
        // contain trailing digits. We return word WITHOUT $; caller appends $.
        // But if is_string, ensure word doesn't contain the $ (it doesn't).
        let _ = &mut word; // no-op to satisfy borrow
        (word, is_string)
    }

    fn peek(&self) -> u8 {
        self.src[self.pos]
    }
    fn advance(&mut self) {
        self.pos += 1;
    }
    fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }
}
