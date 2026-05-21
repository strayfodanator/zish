use crate::parser::ast::Word;
use anyhow::Result;
use std::process::Command;

/// Word expander — handles variable expansion, glob, arithmetic, cmd substitution
pub struct Expander {
    // Could hold local scope in the future
}

impl Expander {
    pub fn new() -> Self {
        Self {}
    }

    /// Expand a word to a single string (no glob splitting)
    pub fn expand_word(&self, word: &Word) -> Result<String> {
        match word {
            Word::Literal(s) => Ok(s.clone()),

            Word::Variable(name) | Word::VarExpand(name) => {
                Ok(self.expand_var(name))
            }

            Word::CmdSubst(stmts) => {
                self.expand_cmd_subst(stmts)
            }

            Word::Arithmetic(expr) => {
                let expanded = self.expand_str(expr)?;
                Ok(eval_arithmetic(&expanded).to_string())
            }

            Word::Concat(parts) => {
                let mut result = String::new();
                for part in parts {
                    result.push_str(&self.expand_word(part)?);
                }
                Ok(result)
            }

            Word::Glob(s) => {
                // In non-glob context, just expand variables inside the string
                self.expand_str(s)
            }

            Word::Tilde => {
                Ok(dirs::home_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "~".to_string()))
            }
        }
    }

    /// Expand a word to possibly multiple strings (glob expansion + word splitting)
    pub fn expand_word_glob(&self, word: &Word, case_sensitive: bool) -> Result<Vec<String>> {
        let expanded = self.expand_word(word)?;

        // Check if it looks like a glob pattern
        if expanded.contains('*') || expanded.contains('?') || expanded.contains('[') {
            let matches = expand_glob(&expanded, case_sensitive)?;
            if !matches.is_empty() {
                return Ok(matches);
            }
            // No matches: return the pattern literally (like bash)
        }

        // Word splitting on IFS
        let ifs = crate::env::get_var("IFS").unwrap_or_else(|| " \t\n".to_string());
        let words = split_on_ifs(&expanded, &ifs);
        Ok(words)
    }

    /// Expand a raw string (handles $VAR, ${VAR}, $(cmd), $((expr)) inline)
    pub fn expand_str(&self, s: &str) -> Result<String> {
        let mut result = String::new();
        let bytes = s.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            match bytes[i] {
                b'$' => {
                    i += 1;
                    match bytes.get(i) {
                        Some(b'(') => {
                            i += 1;
                            if bytes.get(i) == Some(&b'(') {
                                // Arithmetic
                                i += 1;
                                let (expr, end) = extract_until_double_rparen(bytes, i);
                                i = end;
                                let expr = self.expand_str(&expr)?;
                                result.push_str(&eval_arithmetic(&expr).to_string());
                            } else {
                                // Command substitution
                                let (inner, end) = extract_balanced_parens(bytes, i);
                                i = end;
                                let inner = self.expand_str(&inner)?;
                                let output = run_cmd_subst(&inner)?;
                                result.push_str(&output);
                            }
                        }
                        Some(b'{') => {
                            i += 1;
                            let (name, end) = extract_until(bytes, i, b'}');
                            i = end;
                            result.push_str(&self.expand_var_expr(&name));
                        }
                        Some(c) if (*c as char).is_alphanumeric() || *c == b'_' || is_special_var(*c) => {
                            if is_special_var(bytes[i]) {
                                let c = bytes[i] as char;
                                i += 1;
                                result.push_str(&self.expand_var(&c.to_string()));
                            } else {
                                let mut name = String::new();
                                while i < bytes.len()
                                    && ((bytes[i] as char).is_alphanumeric() || bytes[i] == b'_')
                                {
                                    name.push(bytes[i] as char);
                                    i += 1;
                                }
                                result.push_str(&self.expand_var(&name));
                            }
                        }
                        _ => result.push('$'),
                    }
                }
                b'~' if i == 0 => {
                    // Tilde expansion at start
                    i += 1;
                    if bytes.get(i).map(|&c| c == b'/' || i >= bytes.len()).unwrap_or(true) {
                        result.push_str(
                            &dirs::home_dir()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| "~".to_string()),
                        );
                    } else {
                        result.push('~');
                    }
                }
                b'\\' => {
                    i += 1;
                    if let Some(&c) = bytes.get(i) {
                        result.push(c as char);
                        i += 1;
                    }
                }
                c => {
                    result.push(c as char);
                    i += 1;
                }
            }
        }

        Ok(result)
    }

    fn expand_var(&self, name: &str) -> String {
        match name {
            "?" => crate::env::get_var("?").unwrap_or_else(|| "0".to_string()),
            "$" => std::process::id().to_string(),
            "!" => crate::env::get_var("!").unwrap_or_default(),
            "#" => crate::env::get_var("#").unwrap_or_else(|| "0".to_string()),
            "@" | "*" => crate::env::get_var("@").unwrap_or_default(),
            "-" => crate::env::get_var("-").unwrap_or_default(),
            "0" => crate::env::get_var("0").unwrap_or_else(|| "zish".to_string()),
            name => crate::env::get_var(name).unwrap_or_default(),
        }
    }

    /// Expand ${VAR:-default}, ${VAR:=default}, ${VAR:+alt}, ${#VAR}, ${VAR%pattern}, etc.
    fn expand_var_expr(&self, expr: &str) -> String {
        if let Some(name) = expr.strip_prefix('#') {
            // ${#VAR} — length
            let val = self.expand_var(name);
            return val.len().to_string();
        }
        if let Some(rest) = expr.strip_prefix('!') {
            // ${!VAR} — indirect expansion
            let name = self.expand_var(rest);
            return self.expand_var(&name);
        }

        // Find first operator: :-, :=, :+, :?, %, %%, #, ##
        if let Some(idx) = find_var_op(expr) {
            let (name, op, default) = split_var_op(expr, idx);
            let val = self.expand_var(name);
            match op {
                ":-" => if val.is_empty() { default.to_string() } else { val },
                ":=" => {
                    if val.is_empty() {
                        crate::env::set_var(name, default);
                        default.to_string()
                    } else { val }
                }
                ":+" => if val.is_empty() { String::new() } else { default.to_string() },
                ":?" => {
                    if val.is_empty() {
                        eprintln!("zish: {}: {}", name, default);
                        std::process::exit(1);
                    }
                    val
                }
                "%" => strip_suffix_shortest(&val, default),
                "%%" => strip_suffix_longest(&val, default),
                "#" => strip_prefix_shortest(&val, default),
                "##" => strip_prefix_longest(&val, default),
                "/" => val.replacen(default, "", 1),
                "//" => val.replace(default, ""),
                _ => val,
            }
        } else {
            self.expand_var(expr)
        }
    }

    fn expand_cmd_subst(&self, stmts: &[crate::parser::ast::Statement]) -> Result<String> {
        use nix::unistd::{fork, ForkResult};
        use std::os::unix::io::IntoRawFd;

        let (r_owned, w_owned) = nix::unistd::pipe()?;
        let read_fd = r_owned.into_raw_fd();
        let write_fd = w_owned.into_raw_fd();

        match unsafe { fork() }? {
            ForkResult::Child => {
                unsafe { nix::unistd::close(read_fd).ok() };
                nix::unistd::dup2(write_fd, 1).ok();
                unsafe { nix::unistd::close(write_fd).ok() };

                let config = crate::config::Config::load().unwrap_or_default();
                let mut exec = crate::executor::Executor::new(config);
                let mut code = 0;
                for stmt in stmts {
                    code = exec.execute(stmt).unwrap_or(1);
                }
                std::process::exit(code);
            }
            ForkResult::Parent { child } => {
                unsafe { nix::unistd::close(write_fd).ok() };
                let mut output = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    match nix::unistd::read(read_fd, &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => output.extend_from_slice(&buf[..n]),
                    }
                }
                unsafe { nix::unistd::close(read_fd).ok() };
                let _ = nix::sys::wait::waitpid(child, None);
                let mut s = String::from_utf8_lossy(&output).to_string();
                while s.ends_with('\n') { s.pop(); }
                Ok(s)
            }
        }
    }
}

// ─── Arithmetic evaluation ─────────────────────────────────────────────────────

fn eval_arithmetic(expr: &str) -> i64 {
    let expr = expr.trim();

    // Handle parentheses
    if expr.starts_with('(') && expr.ends_with(')') {
        return eval_arithmetic(&expr[1..expr.len() - 1]);
    }

    // Try to find lowest-precedence operator
    // Operators by precedence (low to high): ||, &&, |, ^, &, ==!=, <<=>>= , <<>>, +-, */%, unary
    if let Some(val) = try_binary_op(expr, "||", |a, b| if a != 0 || b != 0 { 1 } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, "&&", |a, b| if a != 0 && b != 0 { 1 } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, "|", |a, b| a | b) { return val; }
    if let Some(val) = try_binary_op(expr, "^", |a, b| a ^ b) { return val; }
    if let Some(val) = try_binary_op(expr, "&", |a, b| a & b) { return val; }
    if let Some(val) = try_binary_op(expr, "==", |a, b| if a == b { 1 } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, "!=", |a, b| if a != b { 1 } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, "<=", |a, b| if a <= b { 1 } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, ">=", |a, b| if a >= b { 1 } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, "<", |a, b| if a < b { 1 } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, ">", |a, b| if a > b { 1 } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, "<<", |a, b| a << b) { return val; }
    if let Some(val) = try_binary_op(expr, ">>", |a, b| a >> b) { return val; }
    if let Some(val) = try_binary_op(expr, "+", |a, b| a + b) { return val; }
    if let Some(val) = try_binary_op(expr, "-", |a, b| a - b) { return val; }
    if let Some(val) = try_binary_op(expr, "*", |a, b| a * b) { return val; }
    if let Some(val) = try_binary_op(expr, "/", |a, b| if b != 0 { a / b } else { 0 }) { return val; }
    if let Some(val) = try_binary_op(expr, "%", |a, b| if b != 0 { a % b } else { 0 }) { return val; }

    // Unary operators
    if let Some(rest) = expr.strip_prefix('-') {
        return -eval_arithmetic(rest.trim());
    }
    if let Some(rest) = expr.strip_prefix('!') {
        return if eval_arithmetic(rest.trim()) == 0 { 1 } else { 0 };
    }
    if let Some(rest) = expr.strip_prefix('~') {
        return !eval_arithmetic(rest.trim());
    }

    // Variable reference in arithmetic
    if expr.chars().all(|c| c.is_alphanumeric() || c == '_') && !expr.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        let val = crate::env::get_var(expr).unwrap_or_default();
        return eval_arithmetic(&val);
    }

    // Parse as integer (decimal, hex, octal)
    if let Some(hex) = expr.strip_prefix("0x").or_else(|| expr.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).unwrap_or(0)
    } else if expr.starts_with('0') && expr.len() > 1 {
        i64::from_str_radix(&expr[1..], 8).unwrap_or(0)
    } else {
        expr.parse().unwrap_or(0)
    }
}

fn try_binary_op(expr: &str, op: &str, f: impl Fn(i64, i64) -> i64) -> Option<i64> {
    // Find the operator at the lowest precedence level (rightmost, outside parens)
    let bytes = expr.as_bytes();
    let mut depth = 0i32;
    let mut i = bytes.len().saturating_sub(op.len());

    loop {
        if bytes[i] == b')' { depth += 1; }
        if bytes[i] == b'(' { depth -= 1; }
        if depth == 0 && bytes[i..].starts_with(op.as_bytes()) {
            // Make sure it's not part of a longer op (e.g., "=" vs "==")
            let before_ok = i == 0 || !matches!(bytes[i-1], b'=' | b'!' | b'<' | b'>' | b'&' | b'|');
            let after_ok = (i + op.len() >= bytes.len()) || !matches!(bytes[i + op.len()], b'=' | b'&' | b'|');
            if before_ok && after_ok {
                let left = expr[..i].trim();
                let right = expr[i + op.len()..].trim();
                if !left.is_empty() && !right.is_empty() {
                    return Some(f(eval_arithmetic(left), eval_arithmetic(right)));
                }
            }
        }
        if i == 0 { break; }
        i -= 1;
    }
    None
}

// ─── Glob expansion ────────────────────────────────────────────────────────────

fn expand_glob(pattern: &str, case_sensitive: bool) -> Result<Vec<String>> {
    let options = glob::MatchOptions {
        case_sensitive,
        require_literal_separator: false,
        require_literal_leading_dot: true,
    };

    let matches: Vec<String> = glob::glob_with(pattern, options)?
        .filter_map(|r| r.ok())
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    Ok(matches)
}

// ─── Word splitting ────────────────────────────────────────────────────────────

fn split_on_ifs(s: &str, ifs: &str) -> Vec<String> {
    if ifs.is_empty() {
        return vec![s.to_string()];
    }
    let ifs_chars: Vec<char> = ifs.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();

    for c in s.chars() {
        if ifs_chars.contains(&c) {
            if !current.is_empty() {
                words.push(current.clone());
                current.clear();
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    if words.is_empty() && !s.is_empty() {
        words.push(s.to_string());
    }
    words
}

// ─── Command substitution via shell ───────────────────────────────────────────

fn run_cmd_subst(cmd: &str) -> Result<String> {
    let output = Command::new("sh").arg("-c").arg(cmd).output()?;
    let mut s = String::from_utf8_lossy(&output.stdout).to_string();
    while s.ends_with('\n') { s.pop(); }
    Ok(s)
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn is_special_var(c: u8) -> bool {
    matches!(c, b'?' | b'$' | b'!' | b'#' | b'@' | b'*' | b'-' | b'0')
}

fn extract_until(bytes: &[u8], start: usize, end: u8) -> (String, usize) {
    let mut i = start;
    let mut result = String::new();
    while i < bytes.len() && bytes[i] != end {
        result.push(bytes[i] as char);
        i += 1;
    }
    (result, i + 1) // skip end char
}

fn extract_balanced_parens(bytes: &[u8], start: usize) -> (String, usize) {
    let mut depth = 1i32;
    let mut i = start;
    let mut result = String::new();
    while i < bytes.len() {
        match bytes[i] {
            b'(' => { depth += 1; result.push('('); }
            b')' => {
                depth -= 1;
                if depth == 0 { i += 1; break; }
                result.push(')');
            }
            c => result.push(c as char),
        }
        i += 1;
    }
    (result, i)
}

fn extract_until_double_rparen(bytes: &[u8], start: usize) -> (String, usize) {
    let mut i = start;
    let mut result = String::new();
    while i < bytes.len() {
        if bytes[i] == b')' && bytes.get(i + 1) == Some(&b')') {
            i += 2;
            break;
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    (result, i)
}

fn find_var_op(expr: &str) -> Option<usize> {
    let ops = [":-", ":=", ":+", ":?", "%%", "##", "%", "#", "//", "/"];
    for op in &ops {
        if let Some(idx) = expr.find(op) {
            return Some(idx);
        }
    }
    None
}

fn split_var_op<'a>(expr: &'a str, idx: usize) -> (&'a str, &'a str, &'a str) {
    let ops = [":-", ":=", ":+", ":?", "%%", "##", "%", "#", "//", "/"];
    for op in &ops {
        if idx + op.len() <= expr.len() && &expr[idx..idx + op.len()] == *op {
            return (&expr[..idx], op, &expr[idx + op.len()..]);
        }
    }
    (expr, "", "")
}

fn strip_suffix_shortest(val: &str, pat: &str) -> String {
    if let Ok(p) = glob::Pattern::new(pat) {
        for i in (0..=val.len()).rev() {
            if p.matches(&val[i..]) {
                return val[..i].to_string();
            }
        }
    }
    val.to_string()
}

fn strip_suffix_longest(val: &str, pat: &str) -> String {
    if let Ok(p) = glob::Pattern::new(pat) {
        for i in 0..=val.len() {
            if p.matches(&val[i..]) {
                return val[..i].to_string();
            }
        }
    }
    val.to_string()
}

fn strip_prefix_shortest(val: &str, pat: &str) -> String {
    if let Ok(p) = glob::Pattern::new(pat) {
        for i in 0..=val.len() {
            if p.matches(&val[..i]) {
                return val[i..].to_string();
            }
        }
    }
    val.to_string()
}

fn strip_prefix_longest(val: &str, pat: &str) -> String {
    if let Ok(p) = glob::Pattern::new(pat) {
        for i in (0..=val.len()).rev() {
            if p.matches(&val[..i]) {
                return val[i..].to_string();
            }
        }
    }
    val.to_string()
}
