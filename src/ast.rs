// AST for parsed statements and expressions.

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(String),
    Var(String), // name includes trailing $ for string vars
    ArrayRef {
        name: String,
        indices: Vec<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    }, // builtin function
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat, // string +
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

/// A single statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Print(PrintExpr),
    Let {
        target: Target,
        expr: Expr,
    },
    Input {
        prompt: String,
        vars: Vec<String>,
    },
    If {
        cond: Expr,
        then: Box<Stmt>,
    },
    Goto(u32),
    Gosub(u32),
    Return,
    For {
        var: String,
        start: Expr,
        limit: Expr,
        step: Option<Expr>,
    },
    Next(Option<String>),
    Rem(String),
    Dim(Vec<(String, Vec<Expr>)>), // (name, dims)
    Data(Vec<Expr>),
    Read(Vec<String>),
    Restore,
    End,
    Stop,
    On {
        expr: Expr,
        targets: Vec<u32>,
        is_gosub: bool,
    },
    // AI extension: send a string to an LLM and print the reply.
    Prompt(Expr),
    // REPL: print a concise list of available commands.
    Help,
    // immediate-mode commands
    Run(Option<String>),
    List(Option<u32>, Option<u32>),
    New,
    Load(String),
    Save(String),
    Clear,
    Empty, // blank line
}

/// What a PRINT statement produces.
///
/// `items[i]` is followed by `seps[i]`, which is the separator that came after
/// it: `Some(Zone)` for `,`, `Some(Tight)` for `;`, or `None` meaning
/// end-of-print (the interpreter emits a newline then). A trailing separator
/// (`PRINT "A";`) yields a final `Some(...)` with no item after it, so no
/// newline is printed.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintExpr {
    pub items: Vec<PrintItem>,
    pub seps: Vec<Option<Sep>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrintItem {
    Expr(Expr),
    Tab(Expr), // TAB(n) cursor positioning
    Spc(Expr), // SPC(n) spaces
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sep {
    Zone,  // comma -> next 16-col zone
    Tight, // semicolon -> no space
}

/// Assignment target.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Var(String),
    Array { name: String, indices: Vec<Expr> },
}
