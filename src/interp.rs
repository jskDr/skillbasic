// Tree-walking interpreter.

use std::collections::HashMap;
use std::io::{BufRead, Write};

use crate::ast::*;
use crate::builtins;
use crate::value::{Value, format_basic_number};

/// Text printed by the `HELP` command: a concise listing of the AI and
/// immediate-mode commands. Kept short so it fits on one screen.
const HELP_TEXT: &str = r#"
Skill BASIC commands
---
AI:       PROMPT <string>
REPL:     RUN [file]   LIST   NEW   LOAD "file"   SAVE "file"   CLEAR
Type a line number to store it; no number runs it immediately."#;
const MAX_ARRAY_ELEMENTS: usize = 1_000_000;
const MAX_CHAT_MESSAGES: usize = 40;
const MAX_CHAT_BYTES: usize = 256 * 1024;

/// Streams passed by the caller; threaded through execution so the caller
/// keeps ownership of stdin/stdout (needed for the REPL).
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub input: &'a mut dyn BufRead,
}

/// Control-flow result of executing one statement.
enum Ctrl {
    Next,        // advance pc by 1
    Jump(usize), // set pc to index
    End,         // END program
    Stop,        // STOP program (prints BREAK message)
}

#[derive(Clone)]
struct ForFrame {
    var: String,
    limit: f64,
    step: f64,
    body: usize, // index to jump back to (first statement of the loop body)
}

enum ArrayVal {
    Num(Vec<f64>, Vec<usize>), // values, dims
    Str(Vec<String>, Vec<usize>),
}

pub struct Interp {
    code: Vec<(u32, Stmt)>,
    next_line: Vec<usize>,
    line_start: HashMap<u32, usize>,
    nums: HashMap<String, f64>,
    strs: HashMap<String, String>,
    arrays: HashMap<String, ArrayVal>,
    for_stack: Vec<ForFrame>,
    gosub_stack: Vec<usize>,
    data: Vec<Value>,
    data_ptr: usize,
    col: usize,                  // current output column (for TAB/zones)
    ai_cfg: crate::ai::AiConfig, // resolved Ollama host/model/default-system for PROMPT
    // Running chat history across all PROMPT calls so the LLM sees prior turns
    // and can answer follow-up questions with full context. Reset on `RUN`
    // (with variables).
    chat: Vec<crate::ai::ChatMessage>,
}

impl Interp {
    pub fn new() -> Self {
        Interp {
            code: Vec::new(),
            next_line: Vec::new(),
            line_start: HashMap::new(),
            nums: HashMap::new(),
            strs: HashMap::new(),
            arrays: HashMap::new(),
            for_stack: Vec::new(),
            gosub_stack: Vec::new(),
            data: Vec::new(),
            data_ptr: 0,
            col: 0,
            ai_cfg: crate::ai::AiConfig::default(),
            chat: Vec::new(),
        }
    }

    /// Install the resolved AI config (Ollama host/model and the default
    /// system prompt from the settings file). `main` calls this once at
    /// startup after reading the settings file and applying env overlays.
    pub fn set_ai_config(&mut self, cfg: crate::ai::AiConfig) {
        self.ai_cfg = cfg;
    }

    /// Load a program (line -> statements) into the flattened code vector
    /// and build the index tables.
    pub fn load(&mut self, prog: &[(u32, Vec<Stmt>)]) {
        self.code.clear();
        self.next_line.clear();
        self.line_start.clear();
        let mut line_starts: Vec<(u32, usize)> = Vec::new();
        for (lineno, stmts) in prog {
            line_starts.push((*lineno, self.code.len()));
            for s in stmts {
                self.code.push((*lineno, s.clone()));
            }
        }
        // next_line[i] = index of the first statement of the line *after* the
        // line containing statement i (or code.len() for the last line).
        let mut next_line_map = vec![self.code.len(); self.code.len()];
        for w in line_starts.windows(2) {
            let start = w[0].1;
            let next_start = w[1].1;
            for entry in next_line_map.iter_mut().take(next_start).skip(start) {
                *entry = next_start;
            }
        }
        self.next_line = next_line_map;
        for (lineno, start) in &line_starts {
            self.line_start.insert(*lineno, *start);
        }
        self.collect_data();
    }

    fn collect_data(&mut self) {
        self.data.clear();
        self.data_ptr = 0;
        // Clone the data items out first to avoid borrowing self during eval.
        let mut data_items: Vec<Vec<Expr>> = Vec::new();
        for (_, stmt) in &self.code {
            if let Stmt::Data(items) = stmt {
                data_items.push(items.clone());
            }
        }
        for items in &data_items {
            for it in items {
                if let Ok(v) = self.eval(it) {
                    self.data.push(v);
                }
            }
        }
    }

    pub fn clear_vars(&mut self) {
        self.nums.clear();
        self.strs.clear();
        self.arrays.clear();
        self.for_stack.clear();
        self.gosub_stack.clear();
        self.data_ptr = 0;
        // Also start a fresh conversation: a new RUN is a new chat session,
        // so a summary prompt like "how many tests passed?" doesn't leak
        // context from a previous run.
        self.chat.clear();
    }

    /// Run the loaded program from the beginning.
    pub fn run(&mut self, io: &mut Io) -> Result<(), String> {
        self.clear_vars();
        let mut pc = 0usize;
        while pc < self.code.len() {
            let (_, stmt) = &self.code[pc];
            let stmt = stmt.clone();
            match self.exec(pc, &stmt, io)? {
                Ctrl::Next => pc += 1,
                Ctrl::Jump(i) => pc = i,
                Ctrl::End => return Ok(()),
                Ctrl::Stop => {
                    let _ = writeln!(io.out);
                    let _ = writeln!(io.out, "BREAK IN {}", self.code[pc].0);
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Execute a single immediate-mode statement (not part of a running program).
    pub fn exec_immediate(&mut self, stmt: &Stmt, io: &mut Io) -> Result<(), String> {
        self.exec(0, stmt, io)?;
        Ok(())
    }

    /// Run a flat list of statements (an immediate-mode line) through the same
    /// pc-driven loop as a program, so FOR...NEXT, IF...THEN, GOSUB/RETURN and
    /// GOTO-into-program all work on one immediate line. Variables are NOT
    /// cleared (immediate assignments persist). The loaded program is kept in
    /// the code vector so GOTO/GOSUB into stored lines resolve; the block is
    /// appended as a synthetic trailing line and removed afterwards.
    pub fn run_block(
        &mut self,
        prog: &[(u32, Vec<Stmt>)],
        block: &[Stmt],
        io: &mut Io,
    ) -> Result<(), String> {
        self.load(prog);
        let start = self.code.len();
        let synth = u32::MAX;
        for s in block {
            self.code.push((synth, s.clone()));
            // the synthetic line has no following line; IF skips to here (end of block)
            self.next_line.push(self.code.len());
        }
        let total = self.code.len();
        let mut pc = start;
        while pc < total {
            let (_, stmt) = self.code[pc].clone();
            match self.exec(pc, &stmt, io)? {
                Ctrl::Next => pc += 1,
                Ctrl::Jump(i) => {
                    // A jump outside the block (into the program) is fine; a jump
                    // past the end ends the block.
                    pc = i;
                }
                Ctrl::End => break,
                Ctrl::Stop => {
                    let _ = writeln!(io.out);
                    let _ = writeln!(io.out, "BREAK");
                    break;
                }
            }
        }
        // restore code to just the program
        self.code.truncate(start);
        self.next_line.truncate(start);
        Ok(())
    }

    fn exec(&mut self, pc: usize, stmt: &Stmt, io: &mut Io) -> Result<Ctrl, String> {
        match stmt {
            Stmt::Empty | Stmt::Rem(_) | Stmt::Data(_) => Ok(Ctrl::Next),
            Stmt::Print(p) => {
                self.exec_print(p, io)?;
                Ok(Ctrl::Next)
            }
            Stmt::Let { target, expr } => {
                let v = self.eval(expr)?;
                self.assign(target, v)?;
                Ok(Ctrl::Next)
            }
            Stmt::Input { prompt, vars } => {
                self.exec_input(prompt, vars, io)?;
                Ok(Ctrl::Next)
            }
            Stmt::If { cond, then } => {
                let v = self.eval(cond)?;
                let truth = v.as_num()? != 0.0;
                if truth {
                    let ctrl = self.exec(pc, then, io)?;
                    match ctrl {
                        Ctrl::Next => Ok(Ctrl::Jump(self.next_line_idx(pc))),
                        other => Ok(other),
                    }
                } else {
                    Ok(Ctrl::Jump(self.next_line_idx(pc)))
                }
            }
            Stmt::Goto(n) => self.jump_line(*n).map(Ctrl::Jump),
            Stmt::Gosub(n) => {
                self.gosub_stack.push(pc + 1);
                self.jump_line(*n).map(Ctrl::Jump)
            }
            Stmt::Return => match self.gosub_stack.pop() {
                Some(i) => Ok(Ctrl::Jump(i)),
                None => Err("?RETURN WITHOUT GOSUB ERROR".into()),
            },
            Stmt::For {
                var,
                start,
                limit,
                step,
            } => {
                let s = self.eval(start)?.as_num()?;
                let lim = self.eval(limit)?.as_num()?;
                let st = match step {
                    Some(e) => self.eval(e)?.as_num()?,
                    None => 1.0,
                };
                self.set_num(var, s);
                // The loop body is the statement immediately after FOR (pc+1),
                // so FOR...NEXT works on a single line as well as across lines.
                self.for_stack.push(ForFrame {
                    var: var.clone(),
                    limit: lim,
                    step: st,
                    body: pc + 1,
                });
                Ok(Ctrl::Next)
            }
            Stmt::Next(opt_var) => self.exec_next(opt_var.as_ref()),
            Stmt::Dim(entries) => {
                for (name, dims) in entries {
                    let ds: Vec<usize> = dims
                        .iter()
                        .map(|e| {
                            self.eval(e)
                                .and_then(|v| v.as_num())
                                .map(|n| (n as i64).max(0) as usize + 1)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    self.create_array(name, &ds)?;
                }
                Ok(Ctrl::Next)
            }
            Stmt::Read(vars) => {
                for v in vars {
                    let val = self.read_data()?;
                    self.set_var(v, val)?;
                }
                Ok(Ctrl::Next)
            }
            Stmt::Restore => {
                self.data_ptr = 0;
                Ok(Ctrl::Next)
            }
            Stmt::End => Ok(Ctrl::End),
            Stmt::Stop => Ok(Ctrl::Stop),
            Stmt::On {
                expr,
                targets,
                is_gosub,
            } => {
                let idx = self.eval(expr)?.as_num()? as i64;
                if idx >= 1 && (idx as usize) <= targets.len() {
                    let target = targets[(idx as usize) - 1];
                    if *is_gosub {
                        self.gosub_stack.push(pc + 1);
                    }
                    self.jump_line(target).map(Ctrl::Jump)
                } else {
                    Ok(Ctrl::Next) // out of range: fall through
                }
            }
            // AI: PROMPT <expr> - send the evaluated string to the LLM and
            // print the reply followed by a newline. The user turn and the
            // assistant reply are both appended to the running chat history,
            // so later PROMPT calls can ask follow-up questions like
            // "how many tests passed?" and the model sees the full context.
            // Errors (e.g. Ollama not running) surface as `?AI ERROR` lines.
            Stmt::Prompt(expr) => {
                let v = self.eval(expr)?;
                let q = match v {
                    Value::Str(s) => s,
                    other => other.as_str()?,
                };
                let reply =
                    crate::ai::complete(&self.ai_cfg, Some(&self.ai_cfg.system), &self.chat, &q)?;
                self.chat.push(crate::ai::ChatMessage::user(q));
                self.chat
                    .push(crate::ai::ChatMessage::assistant(reply.clone()));
                self.trim_chat();
                self.emit(io, &reply);
                self.newline(io);
                Ok(Ctrl::Next)
            }
            // HELP: print a concise command listing. Works in both program
            // and immediate mode (immediate mode routes it through run_block).
            Stmt::Help => {
                self.emit(io, HELP_TEXT);
                self.newline(io);
                Ok(Ctrl::Next)
            }
            Stmt::Run(_)
            | Stmt::List(_, _)
            | Stmt::New
            | Stmt::Load(_)
            | Stmt::Save(_)
            | Stmt::Clear => Err("?SYNTAX ERROR: command not allowed in a program".into()),
        }
    }

    fn next_line_idx(&self, pc: usize) -> usize {
        self.next_line.get(pc).copied().unwrap_or(self.code.len())
    }

    fn jump_line(&self, n: u32) -> Result<usize, String> {
        self.line_start
            .get(&n)
            .copied()
            .ok_or_else(|| format!("?UNDEF'D STATEMENT ERROR IN {}", n))
    }

    fn exec_next(&mut self, opt_var: Option<&String>) -> Result<Ctrl, String> {
        let frame = match self.for_stack.last() {
            Some(f) => f.clone(),
            None => return Err("?NEXT WITHOUT FOR ERROR".into()),
        };
        if let Some(v) = opt_var
            && v != &frame.var
        {
            return Err("?NEXT WITHOUT FOR ERROR".into());
        }
        let cur = self.get_num(&frame.var);
        let new = cur + frame.step;
        self.set_num(&frame.var, new);
        let cont = if frame.step >= 0.0 {
            new <= frame.limit
        } else {
            new >= frame.limit
        };
        if cont {
            Ok(Ctrl::Jump(frame.body))
        } else {
            self.for_stack.pop();
            Ok(Ctrl::Next)
        }
    }

    // ---- PRINT ----
    fn exec_print(&mut self, p: &PrintExpr, io: &mut Io) -> Result<(), String> {
        // Bare PRINT (no items) just emits a newline.
        if p.items.is_empty() {
            self.newline(io);
            return Ok(());
        }
        for i in 0..p.items.len() {
            let item = &p.items[i];
            match item {
                PrintItem::Expr(e) => {
                    let v = self.eval(e)?;
                    self.print_value(&v, io)?;
                }
                PrintItem::Tab(e) => {
                    let n = self.eval(e)?.as_num()? as usize;
                    if self.col < n {
                        let spaces = n - self.col;
                        for _ in 0..spaces {
                            self.emit(io, " ");
                        }
                    }
                }
                PrintItem::Spc(e) => {
                    let n = self.eval(e)?.as_num()? as usize;
                    for _ in 0..n {
                        self.emit(io, " ");
                    }
                }
            }
            match p.seps.get(i).copied().flatten() {
                Some(Sep::Zone) => self.tab_zone(io),
                Some(Sep::Tight) => {}    // nothing
                None => self.newline(io), // end of print list -> newline
            }
        }
        Ok(())
    }

    fn print_value(&mut self, v: &Value, io: &mut Io) -> Result<(), String> {
        match v {
            Value::Num(n) => {
                let s = if *n >= 0.0 {
                    format!(" {} ", format_basic_number(*n))
                } else {
                    format!("{} ", format_basic_number(*n))
                };
                self.emit(io, &s);
            }
            Value::Str(s) => self.emit(io, s),
        }
        Ok(())
    }

    fn tab_zone(&mut self, io: &mut Io) {
        // advance to next multiple of 16
        let target = ((self.col / 16) + 1) * 16;
        while self.col < target {
            self.emit(io, " ");
        }
    }

    fn emit(&mut self, io: &mut Io, s: &str) {
        let _ = io.out.write_all(s.as_bytes());
        for c in s.chars() {
            if c == '\n' {
                self.col = 0;
            } else {
                self.col += 1;
            }
        }
    }

    fn newline(&mut self, io: &mut Io) {
        let _ = io.out.write_all(b"\n");
        self.col = 0;
    }

    /// Public helper so main can route command output (LIST, prompts) through
    /// the same writer and keep the print column in sync.
    pub fn write_out(&mut self, io: &mut Io, s: &str) {
        self.emit(io, s);
    }
    pub fn newline_out(&mut self, io: &mut Io) {
        self.newline(io);
    }

    fn exec_input(&mut self, prompt: &str, vars: &[String], io: &mut Io) -> Result<(), String> {
        if !prompt.is_empty() {
            self.emit(io, prompt);
        } else {
            self.emit(io, "? ");
        }
        let _ = io.out.flush();
        let mut line = String::new();
        if io
            .input
            .read_line(&mut line)
            .map_err(|e| format!("?I/O ERROR: {}", e))?
            == 0
        {
            return Err("?INPUT ERROR: end of input".into());
        }
        let line = line.trim_end_matches(['\n', '\r']);
        let fields: Vec<String> = if vars.len() == 1 {
            vec![line.to_string()]
        } else {
            line.split(',').map(|s| s.trim().to_string()).collect()
        };
        for (i, v) in vars.iter().enumerate() {
            let raw = fields.get(i).map(|s| s.as_str()).unwrap_or("");
            self.set_var_from_input(v, raw)?;
        }
        Ok(())
    }

    fn set_var_from_input(&mut self, name: &str, raw: &str) -> Result<(), String> {
        if name.ends_with('$') {
            self.strs.insert(name.to_string(), raw.to_string());
        } else {
            let n = crate::value::parse_leading_num_pub(raw);
            self.nums.insert(name.to_string(), n);
        }
        Ok(())
    }

    // ---- expression evaluation ----
    fn eval(&mut self, e: &Expr) -> Result<Value, String> {
        match e {
            Expr::Num(n) => Ok(Value::Num(*n)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Var(name) => {
                if name.ends_with('$') {
                    Ok(Value::Str(self.strs.get(name).cloned().unwrap_or_default()))
                } else {
                    Ok(Value::Num(*self.nums.get(name).unwrap_or(&0.0)))
                }
            }
            Expr::ArrayRef { name, indices } => {
                let idx = self.flat_index(name, indices)?;
                self.array_get(name, idx)
            }
            Expr::Unary { op, expr } => {
                let v = self.eval(expr)?;
                match op {
                    UnOp::Neg => Ok(Value::Num(-v.as_num()?)),
                    UnOp::Not => Ok(Value::Num(if v.as_num()? == 0.0 { -1.0 } else { 0.0 })),
                }
            }
            Expr::Binary { op, left, right } => {
                let l = self.eval(left)?;
                let r = self.eval(right)?;
                self.binop(*op, l, r)
            }
            Expr::Call { name, args } => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a)?);
                }
                builtins::apply(name, vals)
            }
        }
    }

    fn binop(&mut self, op: BinOp, l: Value, r: Value) -> Result<Value, String> {
        // string + concatenation
        if op == BinOp::Add && l.is_string() && r.is_string() {
            return Ok(Value::Str(format!("{}{}", l.as_str()?, r.as_str()?)));
        }
        // comparison: if either side is a string, do string comparison
        if l.is_string() || r.is_string() {
            if matches!(
                op,
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
            ) {
                let a = l.as_str()?;
                let b = r.as_str()?;
                let res = match op {
                    BinOp::Eq => a == b,
                    BinOp::Ne => a != b,
                    BinOp::Lt => a < b,
                    BinOp::Gt => a > b,
                    BinOp::Le => a <= b,
                    BinOp::Ge => a >= b,
                    _ => unreachable!(),
                };
                return Ok(Value::Num(if res { -1.0 } else { 0.0 }));
            }
            return Err("?TYPE MISMATCH ERROR".into());
        }
        let a = l.as_num()?;
        let b = r.as_num()?;
        let res = match op {
            BinOp::Add => a + b,
            BinOp::Sub => a - b,
            BinOp::Mul => a * b,
            BinOp::Div => {
                if b == 0.0 {
                    return Err("?DIVISION BY ZERO ERROR".into());
                }
                a / b
            }
            BinOp::Pow => a.powf(b),
            BinOp::Eq => {
                if a == b {
                    -1.0
                } else {
                    0.0
                }
            }
            BinOp::Ne => {
                if a != b {
                    -1.0
                } else {
                    0.0
                }
            }
            BinOp::Lt => {
                if a < b {
                    -1.0
                } else {
                    0.0
                }
            }
            BinOp::Gt => {
                if a > b {
                    -1.0
                } else {
                    0.0
                }
            }
            BinOp::Le => {
                if a <= b {
                    -1.0
                } else {
                    0.0
                }
            }
            BinOp::Ge => {
                if a >= b {
                    -1.0
                } else {
                    0.0
                }
            }
            BinOp::And => {
                if a as i64 != 0 && b as i64 != 0 {
                    -1.0
                } else {
                    0.0
                }
            }
            BinOp::Or => {
                if a as i64 != 0 || b as i64 != 0 {
                    -1.0
                } else {
                    0.0
                }
            }
            BinOp::Concat => return Err("?TYPE MISMATCH ERROR".into()),
        };
        Ok(Value::Num(res))
    }

    // ---- variable / array storage ----
    fn set_num(&mut self, name: &str, n: f64) {
        self.nums.insert(name.to_string(), n);
    }
    fn get_num(&self, name: &str) -> f64 {
        *self.nums.get(name).unwrap_or(&0.0)
    }

    fn set_var(&mut self, name: &str, v: Value) -> Result<(), String> {
        if name.ends_with('$') {
            self.strs.insert(name.to_string(), v.as_str()?);
        } else {
            self.nums.insert(name.to_string(), v.as_num()?);
        }
        Ok(())
    }

    fn assign(&mut self, target: &Target, v: Value) -> Result<(), String> {
        match target {
            Target::Var(name) => self.set_var(name, v),
            Target::Array { name, indices } => {
                let idx = self.flat_index(name, indices)?;
                self.array_set(name, idx, v)
            }
        }
    }

    fn create_array(&mut self, name: &str, dims: &[usize]) -> Result<(), String> {
        let total = dims.iter().try_fold(1usize, |total, dim| {
            total.checked_mul(*dim).filter(|n| *n <= MAX_ARRAY_ELEMENTS)
        });
        let Some(total) = total else {
            return Err(format!(
                "?OUT OF MEMORY ERROR: arrays are limited to {} elements",
                MAX_ARRAY_ELEMENTS
            ));
        };
        if name.ends_with('$') {
            self.arrays.insert(
                name.to_string(),
                ArrayVal::Str(vec![String::new(); total], dims.to_vec()),
            );
        } else {
            self.arrays.insert(
                name.to_string(),
                ArrayVal::Num(vec![0.0; total], dims.to_vec()),
            );
        }
        Ok(())
    }

    fn ensure_array(&mut self, name: &str, ndims: usize) -> Result<(), String> {
        if self.arrays.contains_key(name) {
            return Ok(());
        }
        // default Applesoft auto-dim: 0..10 for each dimension (size 11)
        let dims: Vec<usize> = (0..ndims).map(|_| 11).collect();
        self.create_array(name, &dims)
    }

    fn flat_index(&mut self, name: &str, indices: &[Expr]) -> Result<usize, String> {
        let ndims = indices.len();
        self.ensure_array(name, ndims)?;
        let idxs: Vec<usize> = indices
            .iter()
            .map(|e| {
                self.eval(e)
                    .and_then(|v| v.as_num())
                    .map(|n| n as i64 as usize)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let arr = self.arrays.get(name).unwrap();
        let dims = match arr {
            ArrayVal::Num(_, d) | ArrayVal::Str(_, d) => d.clone(),
        };
        if dims.len() != ndims {
            return Err("?BAD SUBSCRIPT ERROR".into());
        }
        let mut flat = 0usize;
        for i in 0..ndims {
            if idxs[i] >= dims[i] {
                return Err("?BAD SUBSCRIPT ERROR".into());
            }
            flat = flat * dims[i] + idxs[i];
        }
        Ok(flat)
    }

    fn trim_chat(&mut self) {
        while self.chat.len() > MAX_CHAT_MESSAGES
            || self.chat.iter().map(|m| m.content.len()).sum::<usize>() > MAX_CHAT_BYTES
        {
            if self.chat.len() >= 2 {
                self.chat.drain(..2);
            } else {
                self.chat.clear();
            }
        }
    }

    fn array_get(&self, name: &str, idx: usize) -> Result<Value, String> {
        match self.arrays.get(name) {
            Some(ArrayVal::Num(v, _)) => Ok(Value::Num(v[idx])),
            Some(ArrayVal::Str(v, _)) => Ok(Value::Str(v[idx].clone())),
            None => Ok(Value::Num(0.0)),
        }
    }

    fn array_set(&mut self, name: &str, idx: usize, val: Value) -> Result<(), String> {
        match self.arrays.get_mut(name) {
            Some(ArrayVal::Num(v, _)) => {
                v[idx] = val.as_num()?;
                Ok(())
            }
            Some(ArrayVal::Str(v, _)) => {
                v[idx] = val.as_str()?;
                Ok(())
            }
            None => Err("?BAD SUBSCRIPT ERROR".into()),
        }
    }

    fn read_data(&mut self) -> Result<Value, String> {
        if self.data_ptr >= self.data.len() {
            return Err("?OUT OF DATA ERROR".into());
        }
        let v = self.data[self.data_ptr].clone();
        self.data_ptr += 1;
        Ok(v)
    }

    // expose code for LIST and SAVE in main
    pub fn code_lines(&self) -> Vec<(u32, Vec<Stmt>)> {
        let mut out: Vec<(u32, Vec<Stmt>)> = Vec::new();
        for (lineno, stmt) in &self.code {
            if let Some(last) = out.last_mut()
                && last.0 == *lineno
            {
                last.1.push(stmt.clone());
                continue;
            }
            out.push((*lineno, vec![stmt.clone()]));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Parse a multi-line program (lines already in order), run it, and return
    /// captured stdout. `input` is the stdin fed to INPUT statements.
    fn run(src: &str, input: &str) -> String {
        let mut prog: Vec<(u32, Vec<Stmt>)> = Vec::new();
        for raw in src.lines() {
            let line = raw.trim_end();
            if line.trim().is_empty() {
                continue;
            }
            match crate::parser::parse(line) {
                Ok((Some(n), stmts)) => prog.push((n, stmts)),
                Ok((None, _)) => {}
                Err(e) => panic!("parse error in test program: {}", e.msg),
            }
        }
        let mut interp = Interp::new();
        interp.load(&prog);
        let mut out: Vec<u8> = Vec::new();
        let mut input = Cursor::new(input.as_bytes());
        let mut io = Io {
            out: &mut out,
            input: &mut input,
        };
        interp.run(&mut io).expect("runtime error");
        String::from_utf8(out).expect("non-utf8 output")
    }

    fn run_imm(src: &str) -> String {
        run(src, "")
    }

    #[test]
    fn for_next_basic() {
        let out = run_imm("10 FOR I=1 TO 4\n20 PRINT I\n30 NEXT I");
        assert!(out.contains(" 1 "));
        assert!(out.contains(" 2 "));
        assert!(out.contains(" 3 "));
        assert!(out.contains(" 4 "));
    }

    #[test]
    fn for_next_single_line() {
        // FOR...NEXT on one line (colon-separated) must still loop.
        let out = run_imm("10 FOR I=1 TO 5: PRINT I: NEXT");
        assert_eq!(out, " 1 \n 2 \n 3 \n 4 \n 5 \n");
    }

    #[test]
    fn for_next_step_and_negative() {
        // STEP 2 ascending, and descending with negative step.
        let out = run_imm(
            "10 FOR I=0 TO 10 STEP 2\n20 PRINT I;\n30 NEXT I\n40 PRINT\n\
             50 FOR J=3 TO 1 STEP -1\n60 PRINT J;\n70 NEXT J",
        );
        // first loop prints 0 2 4 6 8 10
        assert!(out.starts_with(" 0  2  4  6  8  10 \n"));
        // descending loop prints 3 2 1
        assert!(out.contains(" 3  2  1 "));
    }

    #[test]
    fn gosub_return() {
        let out = run_imm(
            "10 GOSUB 100\n20 PRINT \"A\"\n30 GOSUB 100\n40 PRINT \"B\"\n50 END\n\
             100 PRINT \"SUB\"\n110 RETURN",
        );
        assert_eq!(out, "SUB\nA\nSUB\nB\n");
    }

    #[test]
    fn if_then_branches() {
        let out = run_imm(
            "10 X=5\n20 IF X=5 THEN PRINT \"EQ\"\n30 IF X<>5 THEN PRINT \"NE\"\n\
             40 IF X>3 THEN 60\n50 PRINT \"SHOULD NOT PRINT\"\n60 PRINT \"JUMPED\"",
        );
        assert_eq!(out, "EQ\nJUMPED\n");
    }

    #[test]
    fn dim_2d_array() {
        let out = run_imm(
            "10 DIM M(2,2)\n20 FOR R=0 TO 2\n30 FOR C=0 TO 2\n40 M(R,C)=R*10+C\n\
             50 NEXT C\n60 NEXT R\n70 PRINT M(0,0),M(1,2),M(2,1)",
        );
        // 0 , 12 , 21 in comma zones
        assert!(out.contains(" 0 "));
        assert!(out.contains(" 12 "));
        assert!(out.contains(" 21 "));
    }

    #[test]
    fn data_read_restore() {
        let out =
            run_imm("10 DATA 1,2,3\n20 READ A,B\n30 PRINT A,B\n40 RESTORE\n50 READ C\n60 PRINT C");
        // first read: 1,2 ; after restore read: 1
        assert!(out.contains(" 1 "));
        let lines: Vec<&str> = out.lines().collect();
        // second printed value on first line is 2
        assert!(lines[0].contains(" 2 "));
        // last line prints 1
        assert!(lines[1].trim().starts_with("1"));
    }

    #[test]
    fn on_goto() {
        // ON 2 GOTO 100,200,300 -> jumps to 200
        let out = run_imm(
            "10 N=2\n20 ON N GOTO 100,200,300\n99 PRINT \"FALL\"\n100 PRINT \"HUND\"\n\
             200 PRINT \"TWOHUND\"\n300 PRINT \"THREEHUND\"",
        );
        // Should print TWOHUND then fall through to 300; lines 99 and 100 skipped.
        assert_eq!(out, "TWOHUND\nTHREEHUND\n");
    }

    #[test]
    fn input_numeric_and_string() {
        let out = run("10 INPUT N\n20 INPUT S$\n30 PRINT N, S$", "42\nHI THERE\n");
        // numeric 42 prints as " 42 ", string prints as-is
        assert!(out.contains(" 42 "));
        assert!(out.contains("HI THERE"));
    }

    #[test]
    fn string_concat_and_funcs() {
        let out = run_imm(
            "10 A$=\"AB\"\n20 B$=\"CD\"\n30 PRINT A$+B$\n40 PRINT LEN(A$+B$)\n\
             50 PRINT LEFT$(\"HELLO\",3); RIGHT$(\"HELLO\",2); MID$(\"HELLO\",2,3)",
        );
        assert!(out.contains("ABCD"));
        assert!(out.contains(" 4 ")); // LEN
        assert!(out.contains("HEL")); // LEFT$("HELLO",3)
        assert!(out.contains("LO")); // RIGHT$("HELLO",2)
        assert!(out.contains("ELL")); // MID$("HELLO",2,3)
    }

    #[test]
    fn arithmetic_precedence() {
        let out = run_imm("10 PRINT 2+3*4\n20 PRINT (2+3)*4\n30 PRINT 2^3^2");
        // 2+3*4=14, (2+3)*4=20, 2^3^2 = 2^(3^2)=2^9=512
        assert!(out.contains(" 14 "));
        assert!(out.contains(" 20 "));
        assert!(out.contains(" 512 "));
    }

    #[test]
    fn comparison_truth_is_minus_one() {
        let out = run_imm("10 PRINT (5=5), (5=6)");
        // Applesoft true = -1, false = 0
        assert!(out.contains("-1"));
        assert!(out.contains(" 0 "));
    }

    #[test]
    fn division_by_zero_errors() {
        let result = {
            let mut prog: Vec<(u32, Vec<Stmt>)> = Vec::new();
            {
                let line = "10 PRINT 1/0";
                let (n, s) = crate::parser::parse(line).unwrap();
                prog.push((n.unwrap(), s));
            }
            let mut interp = Interp::new();
            interp.load(&prog);
            let mut out: Vec<u8> = Vec::new();
            let mut input = Cursor::new(&b""[..]);
            let mut io = Io {
                out: &mut out,
                input: &mut input,
            };
            interp.run(&mut io)
        };
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("DIVISION BY ZERO"));
    }

    #[test]
    fn oversized_array_is_rejected() {
        let (_, stmts) = crate::parser::parse("10 DIM A(1000000)").unwrap();
        let mut interp = Interp::new();
        interp.load(&[(10, stmts)]);
        let mut out = Vec::new();
        let mut input = Cursor::new(&b""[..]);
        let mut io = Io {
            out: &mut out,
            input: &mut input,
        };
        let err = interp.run(&mut io).unwrap_err();
        assert!(err.contains("arrays are limited"), "got: {}", err);
    }

    #[test]
    fn chat_history_is_bounded() {
        let mut interp = Interp::new();
        for _ in 0..(MAX_CHAT_MESSAGES + 10) {
            interp.chat.push(crate::ai::ChatMessage::user("question"));
            interp
                .chat
                .push(crate::ai::ChatMessage::assistant("answer"));
        }
        interp.trim_chat();
        assert!(interp.chat.len() <= MAX_CHAT_MESSAGES);
    }

    // ---- PROMPT (AI) ----
    //
    // These tests exercise the wiring without hitting the network: we install
    // an AiConfig pointing at an unroutable host so every call fails fast with
    // a clean `?AI ERROR` (verifying graceful degradation, no panic), and we
    // check the parser builds the right AST shape.

    /// Helper: run a program with the AI config pointed at an unroutable
    /// address so every PROMPT call fails fast with a clean `?AI ERROR`
    /// (verifying graceful degradation, no panic). Returns the runtime Result
    /// so tests can assert on the error message. No env-var mutation needed.
    fn run_ai_err(src: &str) -> Result<(), String> {
        {
            let mut prog: Vec<(u32, Vec<Stmt>)> = Vec::new();
            for line in src.lines() {
                let line = line.trim_end();
                if line.trim().is_empty() {
                    continue;
                }
                let (n, s) = crate::parser::parse(line).unwrap();
                prog.push((n.unwrap(), s));
            }
            let mut interp = Interp::new();
            // Force an unroutable Ollama host so AI calls fail predictably
            // without depending on whether a real Ollama is running on the
            // test machine.
            interp.set_ai_config(crate::ai::AiConfig {
                host: "http://127.0.0.1:1".to_string(),
                model: "__test__".to_string(),
                system: String::new(),
            });
            interp.load(&prog);
            let mut out: Vec<u8> = Vec::new();
            let mut input = Cursor::new(&b""[..]);
            let mut io = Io {
                out: &mut out,
                input: &mut input,
            };
            interp.run(&mut io)
        }
    }

    #[test]
    fn prompt_statement_surfaces_ai_error_when_ollama_unreachable() {
        // The PROMPT statement must fail with a clean ?AI ERROR (no panic)
        // when Ollama is not reachable.
        let result = run_ai_err("10 PROMPT \"anything\"");
        assert!(result.is_err(), "expected an error, got: {:?}", result);
        let msg = result.unwrap_err();
        assert!(msg.starts_with("?AI ERROR"), "got: {}", msg);
    }

    #[test]
    fn prompt_statement_parses_to_prompt_node() {
        // The PROMPT statement should parse into Stmt::Prompt with the string
        // expression as its argument.
        let (_, stmts) = crate::parser::parse("10 PROMPT \"hello\"").unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(&stmts[0], Stmt::Prompt(_)), "got: {:?}", stmts[0]);
    }

    #[test]
    fn bare_prompt_is_a_syntax_error() {
        // `PROMPT` with no argument is meaningless and must be rejected.
        let result = crate::parser::parse("10 PROMPT");
        assert!(result.is_err());
        let msg = result.unwrap_err().msg;
        assert!(msg.contains("PROMPT"), "got: {}", msg);
    }
}
