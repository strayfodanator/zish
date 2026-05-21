pub mod ast;
pub mod lexer;
pub mod grammar;

use anyhow::{bail, Result};
use ast::*;
use lexer::{Lexer, Token, TokenKind};

/// Parse a command line string into an AST
pub fn parse(input: &str) -> Result<Vec<Statement>> {
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    parser.parse_statements()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_kind(&self) -> Option<&TokenKind> {
        self.peek().map(|t| &t.kind)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<&Token> {
        if let Some(t) = self.peek() {
            if &t.kind == kind {
                self.pos += 1;
                return Ok(&self.tokens[self.pos - 1]);
            }
            bail!("expected {:?}, got {:?}", kind, t.kind);
        }
        bail!("unexpected end of input, expected {:?}", kind);
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek_kind(), Some(TokenKind::Newline) | Some(TokenKind::Semicolon)) {
            self.advance();
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn parse_statements(&mut self) -> Result<Vec<Statement>> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at_end() {
            if let Some(stmt) = self.parse_statement()? {
                stmts.push(stmt);
            }
            self.skip_newlines();
        }
        Ok(stmts)
    }

    fn parse_statement(&mut self) -> Result<Option<Statement>> {
        match self.peek_kind() {
            None => Ok(None),
            Some(TokenKind::Newline) | Some(TokenKind::Semicolon) => {
                self.advance();
                Ok(None)
            }
            Some(TokenKind::If) => Ok(Some(self.parse_if()?)),
            Some(TokenKind::While) => Ok(Some(self.parse_while()?)),
            Some(TokenKind::For) => Ok(Some(self.parse_for()?)),
            Some(TokenKind::Case) => Ok(Some(self.parse_case()?)),
            Some(TokenKind::Function) => Ok(Some(self.parse_function_def()?)),
            _ => Ok(Some(self.parse_pipeline_list()?)),
        }
    }

    /// Parse a list of pipelines joined by && / || / ; / &
    fn parse_pipeline_list(&mut self) -> Result<Statement> {
        let mut left = self.parse_pipeline()?;

        loop {
            match self.peek_kind() {
                Some(TokenKind::AndAnd) => {
                    self.advance();
                    self.skip_newlines();
                    let right = self.parse_pipeline()?;
                    left = Statement::And(Box::new(left), Box::new(right));
                }
                Some(TokenKind::OrOr) => {
                    self.advance();
                    self.skip_newlines();
                    let right = self.parse_pipeline()?;
                    left = Statement::Or(Box::new(left), Box::new(right));
                }
                Some(TokenKind::Ampersand) => {
                    self.advance();
                    left = Statement::Background(Box::new(left));
                    // After &, may have another statement
                    self.skip_newlines();
                    if !self.at_end()
                        && !matches!(
                            self.peek_kind(),
                            Some(TokenKind::Fi)
                                | Some(TokenKind::Done)
                                | Some(TokenKind::Esac)
                                | Some(TokenKind::Else)
                                | Some(TokenKind::Elif)
                                | Some(TokenKind::End)
                        )
                    {
                        let next = self.parse_pipeline_list()?;
                        left = Statement::Sequence(Box::new(left), Box::new(next));
                    }
                    break;
                }
                Some(TokenKind::Semicolon) | Some(TokenKind::Newline) => {
                    self.advance();
                    break;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// Parse a pipeline: cmd1 | cmd2 | cmd3 (optionally cmd1 |& cmd2 for stderr)
    fn parse_pipeline(&mut self) -> Result<Statement> {
        let negate = if matches!(self.peek_kind(), Some(TokenKind::Bang)) {
            self.advance();
            true
        } else {
            false
        };

        let mut cmds = vec![self.parse_command()?];
        let mut stderr_merge_flags = vec![false];

        loop {
            match self.peek_kind() {
                Some(TokenKind::Pipe) => {
                    self.advance();
                    self.skip_newlines();
                    cmds.push(self.parse_command()?);
                    stderr_merge_flags.push(false);
                }
                Some(TokenKind::PipeErr) => {
                    self.advance();
                    self.skip_newlines();
                    cmds.push(self.parse_command()?);
                    stderr_merge_flags.push(true);
                }
                _ => break,
            }
        }

        if cmds.len() == 1 && !negate {
            Ok(cmds.remove(0))
        } else {
            Ok(Statement::Pipeline {
                commands: cmds,
                negate,
                stderr_pipes: stderr_merge_flags,
            })
        }
    }

    /// Parse a single command with its redirections and assignments
    fn parse_command(&mut self) -> Result<Statement> {
        // Check for subshell ( ... )
        if matches!(self.peek_kind(), Some(TokenKind::LParen)) {
            return self.parse_subshell();
        }

        // Check for compound { ... }
        if matches!(self.peek_kind(), Some(TokenKind::LBrace)) {
            return self.parse_compound();
        }

        let mut assignments: Vec<(String, Word)> = Vec::new();
        let mut words: Vec<Word> = Vec::new();
        let mut redirects: Vec<Redirect> = Vec::new();

        loop {
            match self.peek_kind() {
                None
                | Some(TokenKind::Semicolon)
                | Some(TokenKind::Newline)
                | Some(TokenKind::Pipe)
                | Some(TokenKind::PipeErr)
                | Some(TokenKind::AndAnd)
                | Some(TokenKind::OrOr)
                | Some(TokenKind::Ampersand)
                | Some(TokenKind::Fi)
                | Some(TokenKind::Done)
                | Some(TokenKind::Esac)
                | Some(TokenKind::Else)
                | Some(TokenKind::Elif)
                | Some(TokenKind::Do)
                | Some(TokenKind::Then)
                | Some(TokenKind::RParen)
                | Some(TokenKind::RBrace) => break,

                Some(TokenKind::Assign) => {
                    let tok = self.advance().unwrap().clone();
                    if let TokenKind::Assign = tok.kind {
                        // tok.value is "VAR=value"
                        let raw = tok.value.clone();
                        let eq = raw.find('=').unwrap();
                        let name = raw[..eq].to_string();
                        let val_str = &raw[eq + 1..];
                        assignments.push((name, Word::from_str(val_str)));
                    }
                }

                Some(TokenKind::Redirect) => {
                    let r = self.parse_redirect()?;
                    redirects.push(r);
                }

                _ => {
                    let tok = self.advance().unwrap().clone();
                    words.push(Word::from_token(&tok));
                }
            }
        }

        if words.is_empty() && !assignments.is_empty() {
            return Ok(Statement::Assignment(assignments));
        }

        Ok(Statement::Command(Command {
            assignments,
            words,
            redirects,
        }))
    }

    fn parse_redirect(&mut self) -> Result<Redirect> {
        let tok = self.advance().unwrap().clone();
        let raw = &tok.value;

        // Patterns: >, >>, <, 2>, 2>>, &>, &>>, 2>&1, <<<, <<
        // fd number prefix
        let (fd, op_and_rest) = if raw.starts_with(|c: char| c.is_ascii_digit()) {
            let fd_end = raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len());
            let fd: i32 = raw[..fd_end].parse().unwrap_or(1);
            (fd, &raw[fd_end..])
        } else {
            (
                if raw.starts_with('<') { 0 } else { 1 },
                raw.as_str(),
            )
        };

        let (kind, target_str) = match op_and_rest {
            s if s.starts_with(">>>") => (RedirectKind::HereString, &s[3..]),
            s if s.starts_with(">>") => (RedirectKind::Append, &s[2..]),
            s if s.starts_with(">&") => {
                let fd2: i32 = s[2..].parse().unwrap_or(1);
                return Ok(Redirect {
                    kind: RedirectKind::DupFd,
                    fd,
                    target: RedirectTarget::Fd(fd2),
                });
            }
            s if s.starts_with("&>>") => (RedirectKind::AppendBoth, &s[3..]),
            s if s.starts_with("&>") => (RedirectKind::RedirectBoth, &s[2..]),
            s if s.starts_with("<<") => (RedirectKind::HereDoc, &s[2..]),
            s if s.starts_with('>') => (RedirectKind::Output, &s[1..]),
            s if s.starts_with('<') => (RedirectKind::Input, &s[1..]),
            _ => (RedirectKind::Output, op_and_rest),
        };

        let target = if target_str.is_empty() {
            // Target is next token
            if let Some(t) = self.advance() {
                RedirectTarget::File(Word::from_token(&t.clone()))
            } else {
                bail!("expected redirect target");
            }
        } else {
            RedirectTarget::File(Word::from_str(target_str))
        };

        Ok(Redirect { kind, fd, target })
    }

    fn parse_subshell(&mut self) -> Result<Statement> {
        self.expect(&TokenKind::LParen)?;
        self.skip_newlines();
        let mut body = Vec::new();
        while !matches!(self.peek_kind(), Some(TokenKind::RParen) | None) {
            if let Some(s) = self.parse_statement()? {
                body.push(s);
            }
            self.skip_newlines();
        }
        self.expect(&TokenKind::RParen)?;
        Ok(Statement::Subshell(body))
    }

    fn parse_compound(&mut self) -> Result<Statement> {
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();
        let mut body = Vec::new();
        while !matches!(self.peek_kind(), Some(TokenKind::RBrace) | None) {
            if let Some(s) = self.parse_statement()? {
                body.push(s);
            }
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Statement::Compound(body))
    }

    fn parse_if(&mut self) -> Result<Statement> {
        self.expect(&TokenKind::If)?;
        let cond = self.parse_pipeline_list()?;
        self.skip_newlines();
        self.expect(&TokenKind::Then)?;
        self.skip_newlines();

        let mut then_body = Vec::new();
        while !matches!(
            self.peek_kind(),
            Some(TokenKind::Else) | Some(TokenKind::Elif) | Some(TokenKind::Fi) | None
        ) {
            if let Some(s) = self.parse_statement()? {
                then_body.push(s);
            }
            self.skip_newlines();
        }

        let mut elseif_clauses = Vec::new();
        let mut else_body = None;

        loop {
            match self.peek_kind() {
                Some(TokenKind::Elif) => {
                    self.advance();
                    let elif_cond = self.parse_pipeline_list()?;
                    self.skip_newlines();
                    self.expect(&TokenKind::Then)?;
                    self.skip_newlines();
                    let mut elif_body = Vec::new();
                    while !matches!(
                        self.peek_kind(),
                        Some(TokenKind::Else)
                            | Some(TokenKind::Elif)
                            | Some(TokenKind::Fi)
                            | None
                    ) {
                        if let Some(s) = self.parse_statement()? {
                            elif_body.push(s);
                        }
                        self.skip_newlines();
                    }
                    elseif_clauses.push((elif_cond, elif_body));
                }
                Some(TokenKind::Else) => {
                    self.advance();
                    self.skip_newlines();
                    let mut eb = Vec::new();
                    while !matches!(self.peek_kind(), Some(TokenKind::Fi) | None) {
                        if let Some(s) = self.parse_statement()? {
                            eb.push(s);
                        }
                        self.skip_newlines();
                    }
                    else_body = Some(eb);
                    break;
                }
                _ => break,
            }
        }

        self.expect(&TokenKind::Fi)?;
        Ok(Statement::If {
            condition: Box::new(cond),
            then_body,
            elseif_clauses,
            else_body,
        })
    }

    fn parse_while(&mut self) -> Result<Statement> {
        let is_until = matches!(self.peek_kind(), Some(TokenKind::Until));
        self.advance(); // consume while/until
        let cond = self.parse_pipeline_list()?;
        self.skip_newlines();
        self.expect(&TokenKind::Do)?;
        self.skip_newlines();
        let mut body = Vec::new();
        while !matches!(self.peek_kind(), Some(TokenKind::Done) | None) {
            if let Some(s) = self.parse_statement()? {
                body.push(s);
            }
            self.skip_newlines();
        }
        self.expect(&TokenKind::Done)?;
        Ok(Statement::While {
            condition: Box::new(cond),
            body,
            until: is_until,
        })
    }

    fn parse_for(&mut self) -> Result<Statement> {
        self.expect(&TokenKind::For)?;
        let var_tok = self.advance().ok_or_else(|| anyhow::anyhow!("expected variable name"))?.clone();
        let var_name = var_tok.value.clone();
        self.skip_newlines();

        // optional `in ITEMS...`
        let items = if matches!(self.peek_kind(), Some(TokenKind::In)) {
            self.advance();
            let mut items = Vec::new();
            while !matches!(
                self.peek_kind(),
                Some(TokenKind::Semicolon)
                    | Some(TokenKind::Newline)
                    | Some(TokenKind::Do)
                    | None
            ) {
                let t = self.advance().unwrap().clone();
                items.push(Word::from_token(&t));
            }
            items
        } else {
            // iterate over "$@"
            vec![Word::Variable("@".to_string())]
        };

        self.skip_newlines();
        self.expect(&TokenKind::Do)?;
        self.skip_newlines();
        let mut body = Vec::new();
        while !matches!(self.peek_kind(), Some(TokenKind::Done) | None) {
            if let Some(s) = self.parse_statement()? {
                body.push(s);
            }
            self.skip_newlines();
        }
        self.expect(&TokenKind::Done)?;
        Ok(Statement::For { var: var_name, items, body })
    }

    fn parse_case(&mut self) -> Result<Statement> {
        self.expect(&TokenKind::Case)?;
        let word_tok = self.advance().ok_or_else(|| anyhow::anyhow!("expected word"))?.clone();
        let word = Word::from_token(&word_tok);
        self.skip_newlines();
        self.expect(&TokenKind::In)?;
        self.skip_newlines();

        let mut arms = Vec::new();
        while !matches!(self.peek_kind(), Some(TokenKind::Esac) | None) {
            // Parse pattern list: PAT1 | PAT2 )
            let mut patterns = Vec::new();
            // optional leading (
            if matches!(self.peek_kind(), Some(TokenKind::LParen)) {
                self.advance();
            }
            loop {
                if let Some(t) = self.advance() {
                    patterns.push(t.value.clone());
                }
                match self.peek_kind() {
                    Some(TokenKind::Pipe) => { self.advance(); }
                    Some(TokenKind::RParen) => { self.advance(); break; }
                    _ => break,
                }
            }
            self.skip_newlines();
            let mut body = Vec::new();
            while !matches!(
                self.peek_kind(),
                Some(TokenKind::CaseSep) | Some(TokenKind::Esac) | None
            ) {
                if let Some(s) = self.parse_statement()? {
                    body.push(s);
                }
                self.skip_newlines();
            }
            if matches!(self.peek_kind(), Some(TokenKind::CaseSep)) {
                self.advance();
            }
            self.skip_newlines();
            arms.push(CaseArm { patterns, body });
        }
        self.expect(&TokenKind::Esac)?;
        Ok(Statement::Case { word, arms })
    }

    fn parse_function_def(&mut self) -> Result<Statement> {
        // function NAME() { ... } OR NAME() { ... }
        if matches!(self.peek_kind(), Some(TokenKind::Function)) {
            self.advance();
        }
        let name_tok = self.advance().ok_or_else(|| anyhow::anyhow!("expected function name"))?.clone();
        let name = name_tok.value.clone();
        // optional ()
        if matches!(self.peek_kind(), Some(TokenKind::LParen)) {
            self.advance();
            if matches!(self.peek_kind(), Some(TokenKind::RParen)) {
                self.advance();
            }
        }
        self.skip_newlines();
        let body = Box::new(self.parse_command()?);
        Ok(Statement::FunctionDef { name, body })
    }
}
