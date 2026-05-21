pub mod builtins;
pub mod job_control;
pub mod pipeline;
pub mod redirects;

use crate::config::Config;
use crate::parser::ast::{Command, Redirect, RedirectKind, RedirectTarget, Statement, Word};
use crate::scripting::expand::Expander;
use anyhow::{bail, Result};
use job_control::JobTable;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{fork, ForkResult, Pid};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub struct Executor {
    pub config: Config,
    pub functions: HashMap<String, Statement>,
    pub aliases: HashMap<String, String>,
    pub jobs: Arc<RwLock<JobTable>>,
    pub last_exit: i32,
    pub last_cmd_duration_ms: u64,
}

impl Executor {
    pub fn new(config: Config) -> Self {
        let aliases = config.aliases.clone();
        Self {
            config,
            functions: HashMap::new(),
            aliases,
            jobs: Arc::new(RwLock::new(JobTable::new())),
            last_exit: 0,
            last_cmd_duration_ms: 0,
        }
    }

    /// Run a string of shell code
    pub fn run_string(&mut self, input: &str) -> Result<i32> {
        let stmts = crate::parser::parse(input)?;
        let mut last = 0;
        for stmt in stmts {
            last = self.execute(&stmt)?;
        }
        Ok(last)
    }

    /// Source (execute) a file
    pub fn run_file(&mut self, path: &std::path::Path, args: &[String]) -> Result<i32> {
        let content = std::fs::read_to_string(path)?;

        // Set positional parameters
        crate::env::set_var("0", path.to_string_lossy().as_ref());
        for (i, arg) in args.iter().enumerate() {
            crate::env::set_var((i + 1).to_string(), arg.as_str());
        }

        self.run_string(&content)
    }

    /// Execute a single AST statement, returning the exit code
    pub fn execute(&mut self, stmt: &Statement) -> Result<i32> {
        match stmt {
            Statement::Command(cmd) => self.execute_command(cmd),
            Statement::Assignment(assignments) => {
                for (name, word) in assignments {
                    let value = self.expand_word(word)?;
                    crate::env::set_var(name.clone(), value);
                }
                Ok(0)
            }
            Statement::Pipeline { commands, negate, stderr_pipes } => {
                let code = pipeline::execute_pipeline(self, commands, stderr_pipes)?;
                Ok(if *negate { if code == 0 { 1 } else { 0 } } else { code })
            }
            Statement::And(left, right) => {
                let code = self.execute(left)?;
                if code == 0 { self.execute(right) } else { Ok(code) }
            }
            Statement::Or(left, right) => {
                let code = self.execute(left)?;
                if code != 0 { self.execute(right) } else { Ok(code) }
            }
            Statement::Background(inner) => {
                self.execute_background(inner)?;
                Ok(0)
            }
            Statement::Sequence(a, b) => {
                self.execute(a)?;
                self.execute(b)
            }
            Statement::Subshell(stmts) => self.execute_subshell(stmts),
            Statement::Compound(stmts) => {
                let mut last = 0;
                for s in stmts { last = self.execute(s)?; }
                Ok(last)
            }
            Statement::If { condition, then_body, elseif_clauses, else_body } => {
                let code = self.execute(condition)?;
                if code == 0 {
                    let mut last = 0;
                    for s in then_body { last = self.execute(s)?; }
                    Ok(last)
                } else {
                    for (cond, body) in elseif_clauses {
                        let c = self.execute(cond)?;
                        if c == 0 {
                            let mut last = 0;
                            for s in body { last = self.execute(s)?; }
                            return Ok(last);
                        }
                    }
                    if let Some(eb) = else_body {
                        let mut last = 0;
                        for s in eb { last = self.execute(s)?; }
                        Ok(last)
                    } else {
                        Ok(code)
                    }
                }
            }
            Statement::While { condition, body, until } => {
                loop {
                    let code = self.execute(condition)?;
                    let should_run = if *until { code != 0 } else { code == 0 };
                    if !should_run { break; }
                    let mut last = 0;
                    for s in body {
                        match self.execute(s) {
                            Err(e) if e.to_string() == "break" => return Ok(last),
                            Err(e) if e.to_string() == "continue" => break,
                            Ok(c) => last = c,
                            Err(e) => return Err(e),
                        }
                    }
                }
                Ok(0)
            }
            Statement::For { var, items, body } => {
                let expanded: Vec<String> = items.iter()
                    .map(|w| self.expand_word(w))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flat_map(|s| s.split_whitespace().map(String::from).collect::<Vec<_>>())
                    .collect();

                let mut last = 0;
                for item in &expanded {
                    crate::env::set_var(var.clone(), item.as_str());
                    for s in body {
                        match self.execute(s) {
                            Err(e) if e.to_string() == "break" => return Ok(last),
                            Err(e) if e.to_string() == "continue" => break,
                            Ok(c) => last = c,
                            Err(e) => return Err(e),
                        }
                    }
                }
                Ok(last)
            }
            Statement::Case { word, arms } => {
                let value = self.expand_word(word)?;
                for arm in arms {
                    for pattern in &arm.patterns {
                        if glob_match(pattern, &value) || pattern == "*" {
                            let mut last = 0;
                            for s in &arm.body { last = self.execute(s)?; }
                            return Ok(last);
                        }
                    }
                }
                Ok(0)
            }
            Statement::FunctionDef { name, body } => {
                self.functions.insert(name.clone(), *body.clone());
                Ok(0)
            }
        }
    }

    pub fn execute_command(&mut self, cmd: &Command) -> Result<i32> {
        // Apply pre-command assignments
        let mut temp_env: Vec<(String, String)> = Vec::new();
        for (name, word) in &cmd.assignments {
            let value = self.expand_word(word)?;
            if cmd.words.is_empty() {
                crate::env::set_var(name.clone(), value.clone());
            } else {
                // Temp env for this command only
                std::env::set_var(name, &value);
                temp_env.push((name.clone(), value));
            }
        }

        if cmd.words.is_empty() {
            return Ok(0);
        }

        // Expand all words
        let mut argv: Vec<String> = Vec::new();
        for word in &cmd.words {
            let expanded = self.expand_word_glob(word)?;
            argv.extend(expanded);
        }

        if argv.is_empty() {
            return Ok(0);
        }

        // Alias expansion (one level)
        let cmd_name = argv[0].clone();
        if let Some(alias_val) = self.aliases.get(&cmd_name).cloned() {
            let mut expanded_input = alias_val.clone();
            if argv.len() > 1 {
                expanded_input.push(' ');
                expanded_input.push_str(&argv[1..].join(" "));
            }
            let result = self.run_string(&expanded_input)?;
            // Restore temp env
            for (k, _) in &temp_env { std::env::remove_var(k); }
            return Ok(result);
        }

        // Built-in check
        if let Some(code) = builtins::try_builtin(self, &argv, &cmd.redirects)? {
            for (k, _) in &temp_env { std::env::remove_var(k); }
            return Ok(code);
        }

        // User-defined function check
        if let Some(func_body) = self.functions.get(&cmd_name).cloned() {
            // Set positional parameters
            for (i, arg) in argv[1..].iter().enumerate() {
                crate::env::set_var((i + 1).to_string(), arg.as_str());
            }
            let result = self.execute(&func_body)?;
            for (k, _) in &temp_env { std::env::remove_var(k); }
            return Ok(result);
        }

        // Auto-cd: if the "command" is a directory and auto_cd is enabled
        if self.config.shell.auto_cd {
            let path = PathBuf::from(&cmd_name);
            if path.is_dir() {
                let code = builtins::cd::builtin_cd(self, &[cmd_name.clone()], &cmd.redirects)?;
                for (k, _) in &temp_env { std::env::remove_var(k); }
                return Ok(code);
            }
        }

        // Fork and exec
        let code = self.fork_exec(&argv, &cmd.redirects)?;
        // Restore temp env
        for (k, _) in &temp_env { std::env::remove_var(k); }
        Ok(code)
    }

    pub fn fork_exec(&mut self, argv: &[String], redirects: &[Redirect]) -> Result<i32> {
        use nix::unistd::execvp;
        use std::ffi::CString;

        let cmd = CString::new(argv[0].as_bytes())?;
        let args: Vec<CString> = argv.iter()
            .map(|a| CString::new(a.as_bytes()).unwrap())
            .collect();

        let start = std::time::Instant::now();

        match unsafe { fork() }? {
            ForkResult::Child => {
                // Apply redirections in child
                redirects::apply_redirects(redirects)?;

                // Restore default signal handlers
                unsafe {
                    use nix::sys::signal::{signal, SigHandler, Signal};
                    let _ = signal(Signal::SIGINT, SigHandler::SigDfl);
                    let _ = signal(Signal::SIGQUIT, SigHandler::SigDfl);
                    let _ = signal(Signal::SIGTSTP, SigHandler::SigDfl);
                }

                execvp(&cmd, &args).map_err(|e| {
                    eprintln!("zish: {}: {}", argv[0], e);
                    std::process::exit(127)
                }).ok();
                std::process::exit(127);
            }
            ForkResult::Parent { child } => {
                let status = self.wait_child(child)?;
                self.last_cmd_duration_ms = start.elapsed().as_millis() as u64;
                crate::env::set_var("?", status.to_string());
                self.last_exit = status;
                Ok(status)
            }
        }
    }

    pub fn wait_child(&self, pid: Pid) -> Result<i32> {
        loop {
            match waitpid(pid, None)? {
                WaitStatus::Exited(_, code) => return Ok(code),
                WaitStatus::Signaled(_, sig, _) => return Ok(128 + sig as i32),
                WaitStatus::Stopped(p, _) => {
                    // Process stopped (Ctrl+Z) — add to jobs
                    self.jobs.write().unwrap().add_stopped(p);
                    return Ok(148); // SIGTSTP exit code
                }
                _ => continue,
            }
        }
    }

    fn execute_background(&mut self, stmt: &Statement) -> Result<Pid> {
        match unsafe { fork() }? {
            ForkResult::Child => {
                let code = self.execute(stmt).unwrap_or(1);
                std::process::exit(code);
            }
            ForkResult::Parent { child } => {
                self.jobs.write().unwrap().add_background(child);
                println!("[{}] {}", self.jobs.read().unwrap().len(), child);
                Ok(child)
            }
        }
    }

    fn execute_subshell(&mut self, stmts: &[Statement]) -> Result<i32> {
        match unsafe { fork() }? {
            ForkResult::Child => {
                let mut code = 0;
                for s in stmts {
                    code = self.execute(s).unwrap_or(1);
                }
                std::process::exit(code);
            }
            ForkResult::Parent { child } => self.wait_child(child),
        }
    }

    /// Expand a word to a string
    pub fn expand_word(&self, word: &Word) -> Result<String> {
        Expander::new().expand_word(word)
    }

    /// Expand a word to multiple strings (for glob expansion)
    pub fn expand_word_glob(&self, word: &Word) -> Result<Vec<String>> {
        Expander::new().expand_word_glob(word, self.config.shell.case_sensitive_completion)
    }
}

/// Simple glob matching for case statements
fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" { return true; }
    // Use glob crate for matching
    let Ok(pat) = glob::Pattern::new(pattern) else { return pattern == value; };
    pat.matches(value)
}
