/// Lexer for zish — tokenizes shell input into tokens for the parser

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Word,
    SingleQuoted,
    DoubleQuoted,
    Variable,
    CmdSubst,
    Arithmetic,
    Assign, // VAR=value

    // Redirects (stored as raw string, e.g. "2>", ">>", "&>")
    Redirect,

    // Operators
    Pipe,     // |
    PipeErr,  // |&
    AndAnd,   // &&
    OrOr,     // ||
    Ampersand, // &
    Bang,     // !

    // Grouping
    LParen,
    RParen,
    LBrace,
    RBrace,

    // Keywords
    If,
    Then,
    Else,
    Elif,
    Fi,
    For,
    In,
    Do,
    Done,
    While,
    Until,
    Case,
    Esac,
    Function,
    End,

    // Separators
    CaseSep,  // ;;
    Semicolon,
    Newline,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub value: String,
    pub span: (usize, usize),
}

pub struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.input.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let c = self.input.get(self.pos).copied();
        self.pos += 1;
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c == b' ' || c == b'\t' {
                self.advance();
            } else {
                break;
            }
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.input.len() {
                break;
            }
            let tok = self.next_token()?;
            tokens.push(tok);
        }
        Ok(tokens)
    }

    fn next_token(&mut self) -> Result<Token> {
        let start = self.pos;
        let c = self.peek().unwrap();

        match c {
            b'\n' => {
                self.advance();
                Ok(Token { kind: TokenKind::Newline, value: "\n".to_string(), span: (start, self.pos) })
            }
            b';' => {
                self.advance();
                if self.peek() == Some(b';') {
                    self.advance();
                    Ok(Token { kind: TokenKind::CaseSep, value: ";;".to_string(), span: (start, self.pos) })
                } else {
                    Ok(Token { kind: TokenKind::Semicolon, value: ";".to_string(), span: (start, self.pos) })
                }
            }
            b'|' => {
                self.advance();
                if self.peek() == Some(b'|') {
                    self.advance();
                    Ok(Token { kind: TokenKind::OrOr, value: "||".to_string(), span: (start, self.pos) })
                } else if self.peek() == Some(b'&') {
                    self.advance();
                    Ok(Token { kind: TokenKind::PipeErr, value: "|&".to_string(), span: (start, self.pos) })
                } else {
                    Ok(Token { kind: TokenKind::Pipe, value: "|".to_string(), span: (start, self.pos) })
                }
            }
            b'&' => {
                self.advance();
                if self.peek() == Some(b'&') {
                    self.advance();
                    Ok(Token { kind: TokenKind::AndAnd, value: "&&".to_string(), span: (start, self.pos) })
                } else if self.peek() == Some(b'>') {
                    // &> redirect
                    let r = self.read_redirect_from("&>")?;
                    Ok(Token { kind: TokenKind::Redirect, value: r, span: (start, self.pos) })
                } else {
                    Ok(Token { kind: TokenKind::Ampersand, value: "&".to_string(), span: (start, self.pos) })
                }
            }
            b'!' => {
                self.advance();
                Ok(Token { kind: TokenKind::Bang, value: "!".to_string(), span: (start, self.pos) })
            }
            b'(' => { self.advance(); Ok(Token { kind: TokenKind::LParen, value: "(".to_string(), span: (start, self.pos) }) }
            b')' => { self.advance(); Ok(Token { kind: TokenKind::RParen, value: ")".to_string(), span: (start, self.pos) }) }
            b'{' => { self.advance(); Ok(Token { kind: TokenKind::LBrace, value: "{".to_string(), span: (start, self.pos) }) }
            b'}' => { self.advance(); Ok(Token { kind: TokenKind::RBrace, value: "}".to_string(), span: (start, self.pos) }) }
            b'#' => {
                // Comment — consume until newline
                while self.peek().map(|c| c != b'\n').unwrap_or(false) {
                    self.advance();
                }
                // Return the newline if present
                self.next_token()
            }
            b'\'' => self.read_single_quoted(start),
            b'"' => self.read_double_quoted(start),
            b'$' => self.read_dollar(start),
            b'<' | b'>' => self.read_redirect(start),
            // Digit followed by > or < = fd redirect
            b'0'..=b'9' if self.peek_at(1).map(|c| c == b'>' || c == b'<').unwrap_or(false) => {
                self.read_redirect(start)
            }
            _ => self.read_word(start),
        }
    }

    fn read_single_quoted(&mut self, start: usize) -> Result<Token> {
        self.advance(); // consume '
        let mut value = String::new();
        loop {
            match self.advance() {
                Some(b'\'') => break,
                Some(b'\\') => {
                    // In single quotes, only \' and \\ are special
                    match self.peek() {
                        Some(b'\'') => { self.advance(); value.push('\''); }
                        Some(b'\\') => { self.advance(); value.push('\\'); }
                        _ => value.push('\\'),
                    }
                }
                Some(c) => value.push(c as char),
                None => bail!("unterminated single-quoted string"),
            }
        }
        Ok(Token { kind: TokenKind::SingleQuoted, value, span: (start, self.pos) })
    }

    fn read_double_quoted(&mut self, start: usize) -> Result<Token> {
        self.advance(); // consume "
        let mut value = String::new();
        loop {
            match self.advance() {
                Some(b'"') => break,
                Some(b'\\') => {
                    match self.peek() {
                        Some(b'"') | Some(b'\\') | Some(b'$') | Some(b'`') | Some(b'\n') => {
                            let c = self.advance().unwrap() as char;
                            if c != '\n' { value.push(c); }
                        }
                        _ => value.push('\\'),
                    }
                }
                Some(c) => value.push(c as char),
                None => bail!("unterminated double-quoted string"),
            }
        }
        Ok(Token { kind: TokenKind::DoubleQuoted, value, span: (start, self.pos) })
    }

    fn read_dollar(&mut self, start: usize) -> Result<Token> {
        self.advance(); // consume $
        match self.peek() {
            Some(b'(') => {
                self.advance();
                if self.peek() == Some(b'(') {
                    // Arithmetic $((
                    self.advance();
                    let expr = self.read_until_double_rparen()?;
                    Ok(Token { kind: TokenKind::Arithmetic, value: expr, span: (start, self.pos) })
                } else {
                    // Command substitution $(
                    let inner = self.read_balanced_parens()?;
                    Ok(Token { kind: TokenKind::CmdSubst, value: inner, span: (start, self.pos) })
                }
            }
            Some(b'{') => {
                self.advance();
                let mut name = String::new();
                loop {
                    match self.advance() {
                        Some(b'}') => break,
                        Some(c) => name.push(c as char),
                        None => bail!("unterminated ${{}}"),
                    }
                }
                Ok(Token { kind: TokenKind::Variable, value: name, span: (start, self.pos) })
            }
            Some(b'@') | Some(b'*') | Some(b'#') | Some(b'?') | Some(b'!')
            | Some(b'0'..=b'9') | Some(b'$') | Some(b'-') => {
                let c = self.advance().unwrap() as char;
                Ok(Token { kind: TokenKind::Variable, value: c.to_string(), span: (start, self.pos) })
            }
            Some(c) if (c as char).is_alphanumeric() || c == b'_' => {
                let mut name = String::new();
                while let Some(c) = self.peek() {
                    if (c as char).is_alphanumeric() || c == b'_' {
                        name.push(self.advance().unwrap() as char);
                    } else {
                        break;
                    }
                }
                Ok(Token { kind: TokenKind::Variable, value: name, span: (start, self.pos) })
            }
            _ => {
                // bare $ — treat as literal
                Ok(Token { kind: TokenKind::Word, value: "$".to_string(), span: (start, self.pos) })
            }
        }
    }

    fn read_until_double_rparen(&mut self) -> Result<String> {
        let mut expr = String::new();
        let mut depth = 1i32;
        loop {
            match self.advance() {
                Some(b'(') => { depth += 1; expr.push('('); }
                Some(b')') => {
                    if self.peek() == Some(b')') && depth == 1 {
                        self.advance();
                        break;
                    }
                    depth -= 1;
                    expr.push(')');
                }
                Some(c) => expr.push(c as char),
                None => bail!("unterminated arithmetic expansion"),
            }
        }
        Ok(expr)
    }

    fn read_balanced_parens(&mut self) -> Result<String> {
        let mut inner = String::new();
        let mut depth = 1i32;
        loop {
            match self.advance() {
                Some(b'(') => { depth += 1; inner.push('('); }
                Some(b')') => {
                    depth -= 1;
                    if depth == 0 { break; }
                    inner.push(')');
                }
                Some(b'\'') => {
                    inner.push('\'');
                    // read until closing quote
                    loop {
                        match self.advance() {
                            Some(b'\'') => { inner.push('\''); break; }
                            Some(c) => inner.push(c as char),
                            None => bail!("unterminated string in command substitution"),
                        }
                    }
                }
                Some(c) => inner.push(c as char),
                None => bail!("unterminated command substitution"),
            }
        }
        Ok(inner)
    }

    fn read_redirect(&mut self, start: usize) -> Result<Token> {
        let mut r = String::new();
        // Consume digit prefix if any
        while let Some(c) = self.peek() {
            if (c as char).is_ascii_digit() {
                r.push(self.advance().unwrap() as char);
            } else {
                break;
            }
        }
        // Consume operator
        match self.peek() {
            Some(b'>') => {
                self.advance(); r.push('>');
                if self.peek() == Some(b'>') { self.advance(); r.push('>'); }
                else if self.peek() == Some(b'&') { self.advance(); r.push('&'); }
            }
            Some(b'<') => {
                self.advance(); r.push('<');
                if self.peek() == Some(b'<') {
                    self.advance(); r.push('<');
                    if self.peek() == Some(b'<') { self.advance(); r.push('<'); }
                }
            }
            _ => {}
        }
        Ok(Token { kind: TokenKind::Redirect, value: r, span: (start, self.pos) })
    }

    fn read_redirect_from(&mut self, prefix: &str) -> Result<String> {
        let mut r = prefix.to_string();
        // skip already-consumed chars
        for _ in 0..prefix.len().saturating_sub(1) { self.advance(); }
        if self.peek() == Some(b'>') { self.advance(); r.push('>'); }
        Ok(r)
    }

    fn read_word(&mut self, start: usize) -> Result<Token> {
        let mut value = String::new();

        loop {
            match self.peek() {
                None
                | Some(b' ')
                | Some(b'\t')
                | Some(b'\n')
                | Some(b';')
                | Some(b'|')
                | Some(b'&')
                | Some(b'(')
                | Some(b')')
                | Some(b'{')
                | Some(b'}')
                | Some(b'<')
                | Some(b'>') => break,

                Some(b'\\') => {
                    self.advance();
                    if self.peek() == Some(b'\n') {
                        // Line continuation
                        self.advance();
                    } else if let Some(c) = self.advance() {
                        value.push(c as char);
                    }
                }

                Some(b'\'') => {
                    let tok = self.read_single_quoted(self.pos)?;
                    value.push_str(&tok.value);
                }

                Some(b'"') => {
                    let tok = self.read_double_quoted(self.pos)?;
                    value.push_str(&tok.value);
                }

                Some(c) => {
                    value.push(c as char);
                    self.advance();
                }
            }
        }

        // Check if it's an assignment (only if value is first word-ish)
        if let Some(eq_pos) = value.find('=') {
            let lhs = &value[..eq_pos];
            if lhs.chars().all(|c| c.is_alphanumeric() || c == '_')
                && lhs.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
            {
                return Ok(Token { kind: TokenKind::Assign, value, span: (start, self.pos) });
            }
        }

        // Keyword check
        let kind = match value.as_str() {
            "if" => TokenKind::If,
            "then" => TokenKind::Then,
            "else" => TokenKind::Else,
            "elif" => TokenKind::Elif,
            "fi" => TokenKind::Fi,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "do" => TokenKind::Do,
            "done" => TokenKind::Done,
            "while" => TokenKind::While,
            "until" => TokenKind::Until,
            "case" => TokenKind::Case,
            "esac" => TokenKind::Esac,
            "function" => TokenKind::Function,
            "end" => TokenKind::End,
            _ => TokenKind::Word,
        };

        Ok(Token { kind, value, span: (start, self.pos) })
    }
}
