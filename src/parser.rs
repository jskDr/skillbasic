// Recursive-descent parser.

use crate::ast::*;
use crate::token::{Kw, SpanTok, Tok};

pub struct Parser {
    toks: Vec<SpanTok>,
    i: usize,
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub msg: String,
}

impl Parser {
    pub fn new(toks: Vec<SpanTok>) -> Self {
        Parser { toks, i: 0 }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }
    fn peek_pos(&self) -> usize {
        self.toks[self.i].pos
    }
    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        self.i += 1;
        t
    }
    fn kw(&self, kw: Kw) -> bool {
        matches!(self.peek(), Tok::Keyword(k) if *k == kw)
    }
    fn tok(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { msg: msg.into() }
    }

    /// Parse a full input line. Returns (optional leading line number, statements).
    /// REM consumes to end-of-line. EOF ends parsing.
    pub fn parse_line(&mut self) -> Result<(Option<u32>, Vec<Stmt>), ParseError> {
        let mut lineno = None;
        if let Tok::Number(n) = self.peek() {
            if *n >= 0.0 && n.fract() == 0.0 {
                lineno = Some(*n as u32);
                self.bump();
            } else {
                return Err(self.err("?SYNTAX ERROR: bad line number"));
            }
        }
        // blank line: no statements
        if self.at_eof() {
            return Ok((lineno, vec![Stmt::Empty]));
        }
        let mut stmts = Vec::new();
        loop {
            let s = self.parse_stmt()?;
            stmts.push(s);
            if self.tok(&Tok::Colon) {
                self.bump();
                if self.at_eof() {
                    break;
                }
                continue;
            }
            break;
        }
        if !self.at_eof() {
            return Err(self.err("?SYNTAX ERROR: expected end of statement"));
        }
        Ok((lineno, stmts))
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match self.peek().clone() {
            Tok::Keyword(kw) => match kw {
                Kw::Print => {
                    self.bump();
                    self.parse_print().map(Stmt::Print)
                }
                Kw::Let => {
                    self.bump();
                    self.parse_let()
                }
                Kw::If => self.parse_if(),
                Kw::Goto => self.parse_goto(),
                Kw::Gosub => self.parse_gosub(),
                Kw::Return => {
                    self.bump();
                    Ok(Stmt::Return)
                }
                Kw::For => self.parse_for(),
                Kw::Next => self.parse_next(),
                Kw::Rem => self.parse_rem(),
                Kw::Dim => self.parse_dim(),
                Kw::Data => self.parse_data(),
                Kw::Read => self.parse_read(),
                Kw::Restore => {
                    self.bump();
                    Ok(Stmt::Restore)
                }
                Kw::End => {
                    self.bump();
                    Ok(Stmt::End)
                }
                Kw::Stop => {
                    self.bump();
                    Ok(Stmt::Stop)
                }
                Kw::On => self.parse_on(),
                Kw::Input => self.parse_input(),
                Kw::Prompt => {
                    self.bump();
                    self.parse_prompt()
                }
                Kw::Run => {
                    self.bump();
                    Ok(Stmt::Run(self.parse_opt_string()))
                }
                Kw::List => self.parse_list(),
                Kw::New => {
                    self.bump();
                    Ok(Stmt::New)
                }
                Kw::Load => {
                    self.bump();
                    Ok(Stmt::Load(self.parse_req_string()?))
                }
                Kw::Save => {
                    self.bump();
                    Ok(Stmt::Save(self.parse_req_string()?))
                }
                Kw::Clear => {
                    self.bump();
                    Ok(Stmt::Clear)
                }
                Kw::Help => {
                    self.bump();
                    Ok(Stmt::Help)
                }
                _ => Err(self.err("?SYNTAX ERROR: unexpected keyword")),
            },
            Tok::Ident(_) => self.parse_let(),
            Tok::Colon => Err(self.err("?SYNTAX ERROR: empty statement")),
            other => Err(self.err(format!("?SYNTAX ERROR: unexpected {:?}", other))),
        }
    }

    fn parse_opt_string(&mut self) -> Option<String> {
        if let Tok::String(s) = self.peek() {
            let s = s.clone();
            self.bump();
            Some(s)
        } else {
            None
        }
    }
    fn parse_req_string(&mut self) -> Result<String, ParseError> {
        if let Tok::String(s) = self.peek() {
            let s = s.clone();
            self.bump();
            Ok(s)
        } else {
            Err(self.err("?SYNTAX ERROR: expected string literal"))
        }
    }

    fn parse_list(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        // LIST [start][-end]]  optional, but we don't fully parse ranges here for immediate
        let mut start = None;
        let mut end = None;
        if let Tok::Number(n) = self.peek() {
            start = Some(*n as u32);
            self.bump();
            if self.tok(&Tok::Minus) {
                self.bump();
                if let Tok::Number(n) = self.peek() {
                    end = Some(*n as u32);
                    self.bump();
                }
            }
        }
        Ok(Stmt::List(start, end))
    }

    fn parse_let(&mut self) -> Result<Stmt, ParseError> {
        // target = expr
        let target = self.parse_target()?;
        if !self.tok(&Tok::Eq) {
            return Err(self.err("?SYNTAX ERROR: expected ="));
        }
        self.bump();
        let expr = self.parse_expr()?;
        Ok(Stmt::Let { target, expr })
    }

    fn parse_target(&mut self) -> Result<Target, ParseError> {
        if let Tok::Ident(name) = self.peek() {
            let name = name.clone();
            self.bump();
            if self.tok(&Tok::LParen) {
                self.bump();
                let mut idx = Vec::new();
                loop {
                    idx.push(self.parse_expr()?);
                    if self.tok(&Tok::Comma) {
                        self.bump();
                        continue;
                    }
                    break;
                }
                if !self.tok(&Tok::RParen) {
                    return Err(self.err("?SYNTAX ERROR: expected )"));
                }
                self.bump();
                Ok(Target::Array { name, indices: idx })
            } else {
                Ok(Target::Var(name))
            }
        } else {
            Err(self.err("?SYNTAX ERROR: expected variable"))
        }
    }

    fn parse_if(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // IF
        let cond = self.parse_expr()?;
        if !self.kw(Kw::Then) {
            return Err(self.err("?SYNTAX ERROR: expected THEN"));
        }
        self.bump();
        // THEN <line number> or THEN <statement>
        if let Tok::Number(n) = self.peek().clone() {
            self.bump();
            return Ok(Stmt::If {
                cond,
                then: Box::new(Stmt::Goto(n as u32)),
            });
        }
        let then = self.parse_stmt()?;
        Ok(Stmt::If {
            cond,
            then: Box::new(then),
        })
    }

    fn parse_goto(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        if let Tok::Number(n) = self.peek().clone() {
            self.bump();
            Ok(Stmt::Goto(n as u32))
        } else {
            Err(self.err("?SYNTAX ERROR: expected line number"))
        }
    }
    fn parse_gosub(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        if let Tok::Number(n) = self.peek().clone() {
            self.bump();
            Ok(Stmt::Gosub(n as u32))
        } else {
            Err(self.err("?SYNTAX ERROR: expected line number"))
        }
    }
    fn parse_next(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        if let Tok::Ident(name) = self.peek() {
            let n = name.clone();
            self.bump();
            Ok(Stmt::Next(Some(n)))
        } else {
            Ok(Stmt::Next(None))
        }
    }
    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        if let Tok::Ident(name) = self.peek() {
            let var = name.clone();
            self.bump();
            if !self.tok(&Tok::Eq) {
                return Err(self.err("?SYNTAX ERROR: expected = in FOR"));
            }
            self.bump();
            let start = self.parse_expr()?;
            if !self.kw(Kw::To) {
                return Err(self.err("?SYNTAX ERROR: expected TO in FOR"));
            }
            self.bump();
            let limit = self.parse_expr()?;
            let step = if self.kw(Kw::Step) {
                self.bump();
                Some(self.parse_expr()?)
            } else {
                None
            };
            Ok(Stmt::For {
                var,
                start,
                limit,
                step,
            })
        } else {
            Err(self.err("?SYNTAX ERROR: expected variable in FOR"))
        }
    }
    fn parse_rem(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // REM
        // The lexer consumes the rest of the line as a single verbatim String
        // token (so comments may contain any character). If present, use it;
        // otherwise the comment was empty.
        let text = match self.peek().clone() {
            Tok::String(s) => {
                self.bump();
                s
            }
            _ => String::new(),
        };
        Ok(Stmt::Rem(text))
    }
    fn parse_dim(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        let mut entries = Vec::new();
        loop {
            if let Tok::Ident(name) = self.peek() {
                let name = name.clone();
                self.bump();
                if !self.tok(&Tok::LParen) {
                    return Err(self.err("?SYNTAX ERROR: expected ( in DIM"));
                }
                self.bump();
                let mut dims = Vec::new();
                loop {
                    dims.push(self.parse_expr()?);
                    if self.tok(&Tok::Comma) {
                        self.bump();
                        continue;
                    }
                    break;
                }
                if !self.tok(&Tok::RParen) {
                    return Err(self.err("?SYNTAX ERROR: expected ) in DIM"));
                }
                self.bump();
                entries.push((name, dims));
                if self.tok(&Tok::Comma) {
                    self.bump();
                    continue;
                }
                break;
            } else {
                return Err(self.err("?SYNTAX ERROR: expected array name in DIM"));
            }
        }
        Ok(Stmt::Dim(entries))
    }
    fn parse_read(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        let mut vars = Vec::new();
        loop {
            if let Tok::Ident(name) = self.peek() {
                vars.push(name.clone());
                self.bump();
            } else {
                return Err(self.err("?SYNTAX ERROR: expected variable in READ"));
            }
            if self.tok(&Tok::Comma) {
                self.bump();
                continue;
            }
            break;
        }
        Ok(Stmt::Read(vars))
    }
    fn parse_data(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        let mut items = Vec::new();
        if self.at_eof() || self.tok(&Tok::Colon) {
            return Ok(Stmt::Data(items));
        }
        loop {
            items.push(self.parse_data_item()?);
            if self.tok(&Tok::Comma) {
                self.bump();
                continue;
            }
            break;
        }
        Ok(Stmt::Data(items))
    }
    fn parse_data_item(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            Tok::Number(n) => {
                self.bump();
                Ok(Expr::Num(n))
            }
            Tok::String(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Minus => {
                self.bump();
                if let Tok::Number(n) = self.peek().clone() {
                    self.bump();
                    Ok(Expr::Num(-n))
                } else {
                    Err(self.err("?SYNTAX ERROR: bad DATA item"))
                }
            }
            // unquoted bareword -> string
            Tok::Ident(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Keyword(k) => {
                self.bump();
                Ok(Expr::Str(kw_text(k).to_string()))
            }
            other => Err(self.err(format!("?SYNTAX ERROR: bad DATA item {:?}", other))),
        }
    }
    fn parse_on(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        let expr = self.parse_expr()?;
        let is_gosub = if self.kw(Kw::Gosub) {
            self.bump();
            true
        } else if self.kw(Kw::Goto) {
            self.bump();
            false
        } else {
            return Err(self.err("?SYNTAX ERROR: expected GOTO/GOSUB in ON"));
        };
        let mut targets = Vec::new();
        loop {
            if let Tok::Number(n) = self.peek() {
                targets.push(*n as u32);
                self.bump();
            } else {
                return Err(self.err("?SYNTAX ERROR: expected line number in ON"));
            }
            if self.tok(&Tok::Comma) {
                self.bump();
                continue;
            }
            break;
        }
        Ok(Stmt::On {
            expr,
            targets,
            is_gosub,
        })
    }
    fn parse_input(&mut self) -> Result<Stmt, ParseError> {
        self.bump();
        // optional "prompt"; or first var
        let mut prompt = String::new();
        if let Tok::String(s) = self.peek() {
            prompt = s.clone();
            self.bump();
            // optional ; or , separator
            if self.tok(&Tok::Semicolon) || self.tok(&Tok::Comma) {
                self.bump();
            }
        }
        let mut vars = Vec::new();
        loop {
            if let Tok::Ident(name) = self.peek() {
                vars.push(name.clone());
                self.bump();
            } else {
                return Err(self.err("?SYNTAX ERROR: expected variable in INPUT"));
            }
            if self.tok(&Tok::Comma) {
                self.bump();
                continue;
            }
            break;
        }
        Ok(Stmt::Input { prompt, vars })
    }

    // ---- PROMPT (AI) ----
    // PROMPT <expr>
    // The expression is evaluated as a string at runtime and sent to the LLM;
    // the reply is printed followed by a newline. A bare `PROMPT` with no
    // argument is a syntax error (there is nothing meaningful to ask).
    fn parse_prompt(&mut self) -> Result<Stmt, ParseError> {
        if self.at_eof() || self.tok(&Tok::Colon) {
            return Err(self.err("?SYNTAX ERROR: PROMPT requires a string expression"));
        }
        let expr = self.parse_expr()?;
        Ok(Stmt::Prompt(expr))
    }

    // ---- PRINT ----
    fn parse_print(&mut self) -> Result<PrintExpr, ParseError> {
        let mut items = Vec::new();
        let mut seps: Vec<Option<Sep>> = Vec::new();
        let terminator = |p: &Parser| p.at_eof() || p.tok(&Tok::Colon);
        if terminator(self) {
            // bare PRINT -> just a newline
            return Ok(PrintExpr { items, seps });
        }
        loop {
            let item = self.parse_print_item()?;
            items.push(item);
            if self.tok(&Tok::Comma) {
                self.bump();
                seps.push(Some(Sep::Zone));
                if terminator(self) {
                    break;
                } // trailing comma -> no newline
                continue;
            } else if self.tok(&Tok::Semicolon) {
                self.bump();
                seps.push(Some(Sep::Tight));
                if terminator(self) {
                    break;
                } // trailing semicolon -> no newline
                continue;
            } else {
                seps.push(None); // end of print -> newline
                break;
            }
        }
        Ok(PrintExpr { items, seps })
    }

    fn parse_print_item(&mut self) -> Result<PrintItem, ParseError> {
        // TAB( expr ) or SPC( expr ) directives
        if let Tok::Ident(name) = self.peek()
            && (name == "TAB" || name == "SPC")
            && self.peek_is_lparen_after()
        {
            let which = name.clone();
            self.bump(); // name
            self.bump(); // (
            let e = self.parse_expr()?;
            if !self.tok(&Tok::RParen) {
                return Err(self.err("?SYNTAX ERROR: expected )"));
            }
            self.bump();
            return Ok(if which == "TAB" {
                PrintItem::Tab(e)
            } else {
                PrintItem::Spc(e)
            });
        }
        Ok(PrintItem::Expr(self.parse_expr()?))
    }

    fn peek_is_lparen_after(&self) -> bool {
        matches!(self.toks.get(self.i + 1).map(|t| &t.tok), Some(Tok::LParen))
    }

    // ---- expressions ----
    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.kw(Kw::Or) {
            self.bump();
            let right = self.parse_and()?;
            left = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }
    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_not()?;
        while self.kw(Kw::And) {
            self.bump();
            let right = self.parse_not()?;
            left = Expr::Binary {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }
    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        if self.kw(Kw::Not) {
            self.bump();
            let e = self.parse_not()?;
            return Ok(Expr::Unary {
                op: UnOp::Not,
                expr: Box::new(e),
            });
        }
        self.parse_rel()
    }
    fn parse_rel(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;
        let op = match self.peek() {
            Tok::Eq => Some(BinOp::Eq),
            Tok::Ne => Some(BinOp::Ne),
            Tok::Lt => Some(BinOp::Lt),
            Tok::Gt => Some(BinOp::Gt),
            Tok::Le => Some(BinOp::Le),
            Tok::Ge => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let right = self.parse_add()?;
            Ok(Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            })
        } else {
            Ok(left)
        }
    }
    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let right = self.parse_mul()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }
    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_pow()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                _ => break,
            };
            self.bump();
            let right = self.parse_pow()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }
    fn parse_pow(&mut self) -> Result<Expr, ParseError> {
        let base = self.parse_unary()?;
        if self.tok(&Tok::Caret) {
            self.bump();
            let exp = self.parse_pow()?; // right associative
            Ok(Expr::Binary {
                op: BinOp::Pow,
                left: Box::new(base),
                right: Box::new(exp),
            })
        } else {
            Ok(base)
        }
    }
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if self.tok(&Tok::Minus) {
            self.bump();
            let e = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnOp::Neg,
                expr: Box::new(e),
            });
        }
        if self.tok(&Tok::Plus) {
            self.bump();
            return self.parse_unary();
        }
        self.parse_primary()
    }
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            Tok::Number(n) => {
                self.bump();
                Ok(Expr::Num(n))
            }
            Tok::String(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Ident(name) => {
                self.bump();
                if self.tok(&Tok::LParen) {
                    self.bump();
                    let mut args = Vec::new();
                    if !self.tok(&Tok::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.tok(&Tok::Comma) {
                                self.bump();
                                continue;
                            }
                            break;
                        }
                    }
                    if !self.tok(&Tok::RParen) {
                        return Err(self.err("?SYNTAX ERROR: expected )"));
                    }
                    self.bump();
                    if crate::builtins::is_builtin(&name) {
                        Ok(Expr::Call { name, args })
                    } else {
                        Ok(Expr::ArrayRef {
                            name,
                            indices: args,
                        })
                    }
                } else {
                    Ok(Expr::Var(name))
                }
            }
            Tok::LParen => {
                self.bump();
                let e = self.parse_expr()?;
                if !self.tok(&Tok::RParen) {
                    return Err(self.err("?SYNTAX ERROR: expected )"));
                }
                self.bump();
                Ok(e)
            }
            other => Err(self.err(format!("?SYNTAX ERROR: unexpected {:?}", other))),
        }
    }
}

fn kw_text(k: Kw) -> &'static str {
    match k {
        Kw::Print => "PRINT",
        Kw::Let => "LET",
        Kw::Input => "INPUT",
        Kw::If => "IF",
        Kw::Then => "THEN",
        Kw::Goto => "GOTO",
        Kw::Gosub => "GOSUB",
        Kw::Return => "RETURN",
        Kw::For => "FOR",
        Kw::To => "TO",
        Kw::Step => "STEP",
        Kw::Next => "NEXT",
        Kw::Rem => "REM",
        Kw::Dim => "DIM",
        Kw::Data => "DATA",
        Kw::Read => "READ",
        Kw::Restore => "RESTORE",
        Kw::End => "END",
        Kw::Stop => "STOP",
        Kw::On => "ON",
        Kw::Run => "RUN",
        Kw::List => "LIST",
        Kw::New => "NEW",
        Kw::Load => "LOAD",
        Kw::Save => "SAVE",
        Kw::Clear => "CLEAR",
        Kw::Prompt => "PROMPT",
        Kw::Help => "HELP",
        Kw::And => "AND",
        Kw::Or => "OR",
        Kw::Not => "NOT",
    }
}

pub fn parse(src: &str) -> Result<(Option<u32>, Vec<Stmt>), ParseError> {
    let toks = crate::lexer::tokenize(src).map_err(|e| ParseError { msg: e.msg })?;
    Parser::new(toks).parse_line()
}
