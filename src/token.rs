// Token types for the Skill BASIC lexer.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // literals
    Number(f64),
    String(String),
    // identifiers (already uppercased; trailing $ marks a string var)
    Ident(String),
    // keywords (uppercased)
    Keyword(Kw),
    // operators / punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Caret, // ^
    Amp,   // & (unused in core, reserved)
    Eq,    // =
    Ne,    // <>
    Lt,    // <
    Gt,    // >
    Le,    // <=
    Ge,    // >=
    LParen,
    RParen,
    Comma,
    Semicolon,
    Colon, // statement separator
    // end of input
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kw {
    // statements / commands
    Print,
    Let,
    Input,
    If,
    Then,
    Goto,
    Gosub,
    Return,
    For,
    To,
    Step,
    Next,
    Rem,
    Dim,
    Data,
    Read,
    Restore,
    End,
    Stop,
    On,
    Run,
    List,
    New,
    Load,
    Save,
    Clear,
    Prompt, // AI: send a string to an LLM and print the reply
    // REPL help
    Help,
    // logical
    And,
    Or,
    Not,
}

impl Kw {
    /// Map an uppercased word to a keyword, if it is one.
    pub fn from_word(w: &str) -> Option<Kw> {
        Some(match w {
            "PRINT" | "?" => Kw::Print,
            "LET" => Kw::Let,
            "INPUT" => Kw::Input,
            "IF" => Kw::If,
            "THEN" => Kw::Then,
            "GOTO" => Kw::Goto,
            "GOSUB" => Kw::Gosub,
            "RETURN" => Kw::Return,
            "FOR" => Kw::For,
            "TO" => Kw::To,
            "STEP" => Kw::Step,
            "NEXT" => Kw::Next,
            "REM" => Kw::Rem,
            "DIM" => Kw::Dim,
            "DATA" => Kw::Data,
            "READ" => Kw::Read,
            "RESTORE" => Kw::Restore,
            "END" => Kw::End,
            "STOP" => Kw::Stop,
            "ON" => Kw::On,
            "RUN" => Kw::Run,
            "LIST" => Kw::List,
            "NEW" => Kw::New,
            "LOAD" => Kw::Load,
            "SAVE" => Kw::Save,
            "CLEAR" => Kw::Clear,
            "PROMPT" | "!" => Kw::Prompt,
            "HELP" => Kw::Help,
            "AND" => Kw::And,
            "OR" => Kw::Or,
            "NOT" => Kw::Not,
            _ => return None,
        })
    }
}

/// A token with its position (byte offset) for error messages.
#[derive(Debug, Clone)]
pub struct SpanTok {
    pub tok: Tok,
    pub pos: usize,
}
