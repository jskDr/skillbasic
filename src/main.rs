// Skill BASIC (simplified) interpreter — REPL and CLI.
// Reserved-for-future fields (token positions, the & operator) intentionally kept.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::exit;

mod ai;
mod ast;
mod builtins;
mod interp;
mod lexer;
mod parser;
mod token;
mod value;

use ast::Stmt;
use interp::{Interp, Io};

/// Project-level AI config file, looked up relative to the current working
/// directory at startup. See `ai::AiConfig::from_file` for the JSON shape.
const SETTINGS_PATH: &str = ".env/settings.json";
const MAX_PROGRAM_BYTES: u64 = 1024 * 1024;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && (args[1] == "-h" || args[1] == "--help") {
        eprintln!("usage: skillbasic [file.bas]   (no args -> interactive REPL)");
        eprintln!("       in the REPL, type HELP for AI/command help");
        eprintln!("       AI config: {} (host, model, system)", SETTINGS_PATH);
        exit(0);
    }

    let mut prog_src: BTreeMap<u32, String> = BTreeMap::new();
    let mut prog: BTreeMap<u32, Vec<Stmt>> = BTreeMap::new();
    let mut interp = Interp::new();

    // Resolve AI config: settings file -> env-var overlays -> install on interp.
    // A malformed file is warned about but not fatal, so the REPL still works;
    // a missing file is silent (the common case when no config is wanted).
    let cfg = match ai::AiConfig::from_file(std::path::Path::new(SETTINGS_PATH)) {
        Ok(cfg) => cfg.with_env_overlays(),
        Err(e) => {
            eprintln!("{} (continuing with default AI config)", e);
            ai::AiConfig::default().with_env_overlays()
        }
    };
    interp.set_ai_config(cfg);

    let stdin = io::stdin();
    let mut stdin_lock = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    // If a file was given, load it and run immediately.
    if args.len() > 1 {
        match load_file(&args[1], &mut prog, &mut prog_src) {
            Ok(()) => {
                let mut io = Io {
                    out: &mut stdout,
                    input: &mut stdin_lock,
                };
                run_program(&mut interp, &prog, &mut io);
            }
            Err(e) => {
                eprintln!("{}", e);
                exit(1);
            }
        }
        return;
    }

    // Interactive REPL.
    let is_tty = io::stdout().is_terminal();
    if is_tty {
        boot_screen(&mut stdout);
    }
    loop {
        write!(stdout, "> ").ok();
        stdout.flush().ok();
        let mut line = String::new();
        match stdin_lock.read_line(&mut line) {
            Ok(0) => {
                writeln!(stdout).ok();
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let mut io = Io {
            out: &mut stdout,
            input: &mut stdin_lock,
        };
        match parser::parse(trimmed) {
            Ok((Some(lineno), stmts)) => {
                if stmts.len() == 1 && matches!(stmts[0], Stmt::Empty) {
                    prog.remove(&lineno);
                    prog_src.remove(&lineno);
                } else {
                    prog.insert(lineno, stmts);
                    prog_src.insert(lineno, trimmed.to_string());
                }
            }
            Ok((None, stmts)) => {
                if let Err(e) =
                    exec_immediate(&mut interp, &mut prog, &mut prog_src, &mut io, &stmts)
                {
                    eprintln!("{}", e);
                }
            }
            Err(e) => eprintln!("{}", e.msg),
        }
    }
}

/// Print a clean Skill BASIC banner (only when stdout is a terminal). This is
/// the modern REPL greeting: a clear screen, the product name and version, and
/// a one-line hint pointing at the `HELP` command. It uses the terminal's own
/// colors (no forced green-on-black CRT aesthetic) so it fits the user's theme.
fn boot_screen<W: Write>(out: &mut W) {
    let _ = write!(out, "\x1b[2J\x1b[H");
    let _ = writeln!(out, "Skill BASIC v0.1.0");
    let _ = writeln!(out, "Type HELP for commands, or start typing a program.");
    let _ = writeln!(out);
    let _ = out.flush();
}

/// Execute a list of immediate-mode statements (no leading line number).
///
/// Command statements (RUN/LIST/NEW/LOAD/SAVE/CLEAR) are handled inline; any run
/// of ordinary statements around them is executed as a block through the
/// interpreter's loop so FOR...NEXT etc. work on one immediate line.
fn exec_immediate(
    interp: &mut Interp,
    prog: &mut BTreeMap<u32, Vec<Stmt>>,
    prog_src: &mut BTreeMap<u32, String>,
    io: &mut Io,
    stmts: &[Stmt],
) -> Result<(), String> {
    let mut block: Vec<Stmt> = Vec::new();
    macro_rules! flush {
        () => {{
            if !block.is_empty() {
                interp.run_block(&flatten(prog), &block, io)?;
                block.clear();
            }
        }};
    }
    for s in stmts {
        match s {
            Stmt::Run(opt) => {
                flush!();
                if let Some(path) = opt {
                    prog.clear();
                    prog_src.clear();
                    load_file(path, prog, prog_src)?;
                    run_program(interp, prog, io);
                } else {
                    run_program(interp, prog, io);
                }
            }
            Stmt::List(start, end) => {
                flush!();
                list_program(prog_src, *start, *end, interp, io);
            }
            Stmt::New => {
                flush!();
                prog.clear();
                prog_src.clear();
                interp.load(&[]);
            }
            Stmt::Load(path) => {
                flush!();
                prog.clear();
                prog_src.clear();
                load_file(path, prog, prog_src)?;
                interp.load(&flatten(prog));
            }
            Stmt::Save(path) => {
                flush!();
                let mut text = String::new();
                for src in prog_src.values() {
                    text.push_str(src);
                    text.push('\n');
                }
                fs::write(path, text).map_err(|e| format!("?I/O ERROR: {}", e))?;
            }
            Stmt::Clear => {
                flush!();
                interp.clear_vars();
            }
            other => {
                block.push(other.clone());
            }
        }
    }
    flush!();
    Ok(())
}

/// Run the stored program.
fn run_program(interp: &mut Interp, prog: &BTreeMap<u32, Vec<Stmt>>, io: &mut Io) -> bool {
    interp.load(&flatten(prog));
    match interp.run(io) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("{}", e);
            false
        }
    }
}

fn list_program(
    prog_src: &BTreeMap<u32, String>,
    start: Option<u32>,
    end: Option<u32>,
    interp: &mut Interp,
    io: &mut Io,
) {
    for (lineno, src) in prog_src.iter() {
        if let Some(s) = start
            && *lineno < s
        {
            continue;
        }
        if let Some(e) = end
            && *lineno > e
        {
            continue;
        }
        interp.write_out(io, src);
        interp.newline_out(io);
    }
}

/// Read a .bas file into the program store. Each non-blank line must begin with
/// a line number; non-numbered lines are ignored.
fn load_file(
    path: &str,
    prog: &mut BTreeMap<u32, Vec<Stmt>>,
    prog_src: &mut BTreeMap<u32, String>,
) -> Result<(), String> {
    let size = fs::metadata(path)
        .map_err(|e| format!("?I/O ERROR: {}", e))?
        .len();
    if size > MAX_PROGRAM_BYTES {
        return Err(format!(
            "?I/O ERROR: program exceeds the {} byte limit",
            MAX_PROGRAM_BYTES
        ));
    }
    let text = fs::read_to_string(path).map_err(|e| format!("?I/O ERROR: {}", e))?;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        match parser::parse(line) {
            Ok((Some(lineno), stmts)) => {
                prog.insert(lineno, stmts);
                prog_src.insert(lineno, line.to_string());
            }
            Ok((None, _)) => { /* ignore non-numbered lines in files */ }
            Err(e) => return Err(format!("{}: {}", path, e.msg)),
        }
    }
    Ok(())
}

/// Flatten the line->statements map into the vector the interpreter expects.
fn flatten(prog: &BTreeMap<u32, Vec<Stmt>>) -> Vec<(u32, Vec<Stmt>)> {
    prog.iter().map(|(n, s)| (*n, s.clone())).collect()
}
