use std::io::{BufRead, Write};

use rustyline::error::ReadlineError;

pub enum ReadOutcome {
    Line(String),
    Eof,
    Interrupted,
}

pub trait LineInput {
    fn read_line(&mut self, prompt: &str, out: &mut dyn Write) -> Result<ReadOutcome, String>;
}

pub struct TerminalLineInput {
    editor: rustyline::DefaultEditor,
}

impl TerminalLineInput {
    pub fn new() -> Result<Self, String> {
        let editor = rustyline::DefaultEditor::new()
            .map_err(|e| format!("?I/O ERROR: could not initialize line editor: {}", e))?;
        Ok(TerminalLineInput { editor })
    }
}

impl LineInput for TerminalLineInput {
    fn read_line(&mut self, prompt: &str, _out: &mut dyn Write) -> Result<ReadOutcome, String> {
        match self.editor.readline(prompt) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    self.editor.add_history_entry(line.as_str()).map_err(|e| {
                        format!("?I/O ERROR: could not update input history: {}", e)
                    })?;
                }
                Ok(ReadOutcome::Line(line))
            }
            Err(ReadlineError::Eof) => Ok(ReadOutcome::Eof),
            Err(ReadlineError::Interrupted) => Ok(ReadOutcome::Interrupted),
            Err(e) => Err(format!("?I/O ERROR: line editor failed: {}", e)),
        }
    }
}

pub struct BufferedLineInput<R> {
    reader: R,
}

impl<R> BufferedLineInput<R> {
    pub fn new(reader: R) -> Self {
        BufferedLineInput { reader }
    }
}

impl<R: BufRead> LineInput for BufferedLineInput<R> {
    fn read_line(&mut self, prompt: &str, out: &mut dyn Write) -> Result<ReadOutcome, String> {
        out.write_all(prompt.as_bytes())
            .and_then(|_| out.flush())
            .map_err(|e| format!("?I/O ERROR: {}", e))?;

        let mut bytes = Vec::new();
        let count = self
            .reader
            .read_until(b'\n', &mut bytes)
            .map_err(|e| format!("?I/O ERROR: {}", e))?;
        if count == 0 {
            return Ok(ReadOutcome::Eof);
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }
        Ok(ReadOutcome::Line(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn buffered_input_preserves_korean_text() {
        let mut input = BufferedLineInput::new(Cursor::new("안녕하세요\n".as_bytes()));
        let mut out = Vec::new();
        let ReadOutcome::Line(line) = input.read_line("YOU: ", &mut out).unwrap() else {
            panic!("expected a line");
        };
        assert_eq!(line, "안녕하세요");
        assert_eq!(out, b"YOU: ");
    }

    #[test]
    fn buffered_input_replaces_invalid_utf8_without_stopping() {
        let mut input = BufferedLineInput::new(Cursor::new(b"hello \xffworld\n"));
        let mut out = Vec::new();
        let ReadOutcome::Line(line) = input.read_line("", &mut out).unwrap() else {
            panic!("expected a line");
        };
        assert_eq!(line, "hello \u{fffd}world");
    }
}
