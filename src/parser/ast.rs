/// AST node types for zish

#[derive(Debug, Clone)]
pub enum Statement {
    /// Simple command with optional assignments and redirects
    Command(Command),
    /// VAR=value ... (no command)
    Assignment(Vec<(String, Word)>),
    /// cmd1 | cmd2 | cmd3
    Pipeline {
        commands: Vec<Statement>,
        negate: bool,
        stderr_pipes: Vec<bool>,
    },
    /// cmd1 && cmd2
    And(Box<Statement>, Box<Statement>),
    /// cmd1 || cmd2
    Or(Box<Statement>, Box<Statement>),
    /// cmd &
    Background(Box<Statement>),
    /// stmt1 ; stmt2 (implicit sequencing)
    Sequence(Box<Statement>, Box<Statement>),
    /// ( stmts )
    Subshell(Vec<Statement>),
    /// { stmts }
    Compound(Vec<Statement>),
    /// if condition; then body; [elif...] [else...] fi
    If {
        condition: Box<Statement>,
        then_body: Vec<Statement>,
        elseif_clauses: Vec<(Statement, Vec<Statement>)>,
        else_body: Option<Vec<Statement>>,
    },
    /// while condition; do body; done
    While {
        condition: Box<Statement>,
        body: Vec<Statement>,
        until: bool,
    },
    /// for VAR in ITEMS; do body; done
    For {
        var: String,
        items: Vec<Word>,
        body: Vec<Statement>,
    },
    /// case WORD in PAT) body;; esac
    Case {
        word: Word,
        arms: Vec<CaseArm>,
    },
    /// function NAME() { body }
    FunctionDef {
        name: String,
        body: Box<Statement>,
    },
}

#[derive(Debug, Clone)]
pub struct Command {
    /// Pre-command variable assignments (VAR=value cmd)
    pub assignments: Vec<(String, Word)>,
    /// Command words (argv)
    pub words: Vec<Word>,
    /// I/O redirections
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone)]
pub struct CaseArm {
    pub patterns: Vec<String>,
    pub body: Vec<Statement>,
}

/// A "word" — can be a literal, variable, glob, command substitution, etc.
#[derive(Debug, Clone)]
pub enum Word {
    Literal(String),
    Variable(String),
    /// $VAR or ${VAR}
    VarExpand(String),
    /// $(cmd) or `cmd`
    CmdSubst(Vec<Statement>),
    /// $((expr))
    Arithmetic(String),
    /// Concatenation of multiple parts
    Concat(Vec<Word>),
    /// Glob pattern
    Glob(String),
    /// ~/ expansion
    Tilde,
}

impl Word {
    pub fn from_str(s: &str) -> Self {
        if s.is_empty() {
            return Word::Literal(String::new());
        }
        // Quick check for common cases
        if !s.contains('$') && !s.contains('~') && !s.contains('*')
            && !s.contains('?') && !s.contains('[')
        {
            return Word::Literal(s.to_string());
        }
        Word::Glob(s.to_string()) // will be further expanded at runtime
    }

    pub fn from_token(tok: &super::lexer::Token) -> Self {
        use super::lexer::TokenKind;
        match &tok.kind {
            TokenKind::Word => Word::from_str(&tok.value),
            TokenKind::SingleQuoted => Word::Literal(tok.value.clone()),
            TokenKind::DoubleQuoted => Word::Glob(tok.value.clone()), // $-expand inside
            TokenKind::Variable => Word::VarExpand(tok.value.clone()),
            TokenKind::CmdSubst => {
                // Parse the inner string
                let inner = crate::parser::parse(&tok.value).unwrap_or_default();
                Word::CmdSubst(inner)
            }
            TokenKind::Arithmetic => Word::Arithmetic(tok.value.clone()),
            _ => Word::Literal(tok.value.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Redirect {
    pub kind: RedirectKind,
    pub fd: i32,
    pub target: RedirectTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RedirectKind {
    /// >
    Output,
    /// >>
    Append,
    /// <
    Input,
    /// &>
    RedirectBoth,
    /// &>>
    AppendBoth,
    /// 2>&1
    DupFd,
    /// <<EOF
    HereDoc,
    /// <<<
    HereString,
}

#[derive(Debug, Clone)]
pub enum RedirectTarget {
    File(Word),
    Fd(i32),
}
