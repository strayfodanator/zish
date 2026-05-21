pub mod cd;
pub mod history_builtin;
pub mod jobs_builtin;

use crate::executor::Executor;
use crate::parser::ast::Redirect;
use anyhow::Result;

/// Try to execute a built-in command. Returns Some(exit_code) if handled, None if not a builtin.
pub fn try_builtin(exec: &mut Executor, argv: &[String], redirects: &[Redirect]) -> Result<Option<i32>> {
    let name = argv[0].as_str();
    let code = match name {
        "cd" => cd::builtin_cd(exec, argv, redirects)?,
        "pwd" => { println!("{}", crate::env::get_cwd()); 0 }
        "echo" => builtin_echo(argv),
        "printf" => builtin_printf(argv),
        "export" => builtin_export(exec, argv),
        "unset" => builtin_unset(exec, argv),
        "alias" => builtin_alias(exec, argv),
        "unalias" => builtin_unalias(exec, argv),
        "source" | "." => builtin_source(exec, argv)?,
        "exit" | "logout" => builtin_exit(argv),
        "true" => 0,
        "false" => 1,
        ":" => 0,
        "set" => builtin_set(exec, argv),
        "return" => {
            let code: i32 = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            anyhow::bail!("return:{}", code)
        }
        "break" => anyhow::bail!("break"),
        "continue" => anyhow::bail!("continue"),
        "type" | "which" => builtin_type(exec, argv),
        "hash" => 0, // no-op (bash compat)
        "jobs" => jobs_builtin::builtin_jobs(exec, argv)?,
        "fg" => jobs_builtin::builtin_fg(exec, argv)?,
        "bg" => jobs_builtin::builtin_bg(exec, argv)?,
        "history" => history_builtin::builtin_history(exec, argv)?,
        "test" | "[" => builtin_test(argv),
        "read" => builtin_read(argv)?,
        "eval" => builtin_eval(exec, argv)?,
        "exec" => builtin_exec(argv)?,
        "umask" => builtin_umask(argv),
        "ulimit" => 0, // stub
        "wait" => builtin_wait(exec, argv)?,
        "builtin" => {
            // Execute next arg as builtin regardless of alias
            if argv.len() > 1 {
                let rest = argv[1..].to_vec();
                return try_builtin(exec, &rest, redirects);
            }
            0
        }
        "command" => {
            // Skip functions/aliases and run directly
            if argv.len() > 1 {
                let rest = argv[1..].to_vec();
                exec.fork_exec(&rest, redirects)?
            } else { 0 }
        }
        _ => return Ok(None),
    };
    Ok(Some(code))
}

fn builtin_echo(argv: &[String]) -> i32 {
    let mut no_newline = false;
    let mut interpret_escapes = false;
    let mut start = 1;

    // Parse flags
    while let Some(arg) = argv.get(start) {
        if arg == "-n" { no_newline = true; start += 1; }
        else if arg == "-e" { interpret_escapes = true; start += 1; }
        else if arg == "-E" { interpret_escapes = false; start += 1; }
        else { break; }
    }

    let output = argv[start..].join(" ");
    let output = if interpret_escapes { interpret_escape_sequences(&output) } else { output };

    if no_newline { print!("{}", output); } else { println!("{}", output); }
    0
}

fn interpret_escape_sequences(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('0') => result.push('\0'),
                Some('a') => result.push('\x07'),
                Some('b') => result.push('\x08'),
                Some('e') => result.push('\x1b'),
                Some(c) => { result.push('\\'); result.push(c); }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn builtin_printf(argv: &[String]) -> i32 {
    if argv.len() < 2 { return 1; }
    // Basic printf: just handle %s, %d, %f, \n
    let fmt = &argv[1];
    let mut args_iter = argv[2..].iter();
    let mut output = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('s') => output.push_str(args_iter.next().map(|s| s.as_str()).unwrap_or("")),
                Some('d') | Some('i') => {
                    let n: i64 = args_iter.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    output.push_str(&n.to_string());
                }
                Some('f') => {
                    let f: f64 = args_iter.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                    output.push_str(&format!("{:.6}", f));
                }
                Some('%') => output.push('%'),
                Some(c) => { output.push('%'); output.push(c); }
                None => output.push('%'),
            }
        } else if c == '\\' {
            match chars.next() {
                Some('n') => output.push('\n'),
                Some('t') => output.push('\t'),
                Some('\\') => output.push('\\'),
                Some(c) => { output.push('\\'); output.push(c); }
                None => output.push('\\'),
            }
        } else {
            output.push(c);
        }
    }
    print!("{}", output);
    0
}

fn builtin_export(exec: &mut Executor, argv: &[String]) -> i32 {
    if argv.len() == 1 {
        // Print all exported vars
        let env = crate::env::SHELL_ENV.read().unwrap();
        for (k, v) in env.exported_vars() {
            println!("export {}={}", k, v);
        }
        return 0;
    }
    for arg in &argv[1..] {
        if let Some(eq) = arg.find('=') {
            let name = arg[..eq].to_string();
            let val = arg[eq + 1..].to_string();
            crate::env::export_var(name, Some(val));
        } else {
            crate::env::export_var(arg.clone(), None);
        }
    }
    0
}

fn builtin_unset(exec: &mut Executor, argv: &[String]) -> i32 {
    for arg in &argv[1..] {
        crate::env::unset_var(arg);
        exec.functions.remove(arg);
    }
    0
}

fn builtin_alias(exec: &mut Executor, argv: &[String]) -> i32 {
    if argv.len() == 1 {
        for (k, v) in &exec.aliases {
            println!("alias {}='{}'", k, v);
        }
        return 0;
    }
    for arg in &argv[1..] {
        if let Some(eq) = arg.find('=') {
            let name = arg[..eq].to_string();
            let val = arg[eq + 1..].trim_matches('\'').trim_matches('"').to_string();
            exec.aliases.insert(name, val);
        } else {
            if let Some(v) = exec.aliases.get(arg.as_str()) {
                println!("alias {}='{}'", arg, v);
            } else {
                eprintln!("zish: alias: {}: not found", arg);
                return 1;
            }
        }
    }
    0
}

fn builtin_unalias(exec: &mut Executor, argv: &[String]) -> i32 {
    if argv.get(1).map(|s| s == "-a").unwrap_or(false) {
        exec.aliases.clear();
        return 0;
    }
    for arg in &argv[1..] {
        exec.aliases.remove(arg.as_str());
    }
    0
}

fn builtin_source(exec: &mut Executor, argv: &[String]) -> Result<i32> {
    if argv.len() < 2 {
        anyhow::bail!("source: filename required");
    }
    let path = std::path::PathBuf::from(&argv[1]);
    let path = if path.exists() { path } else {
        // Search in PATH
        let found = std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
            .map(|p| p.join(&argv[1]))
            .find(|p| p.exists());
        found.ok_or_else(|| anyhow::anyhow!("source: {}: file not found", argv[1]))?
    };
    exec.run_file(&path, &argv[2..])
}

fn builtin_exit(argv: &[String]) -> i32 {
    let code: i32 = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    std::process::exit(code);
}

fn builtin_set(exec: &mut Executor, argv: &[String]) -> i32 {
    // Minimal set: -e (errexit), -x (xtrace), -u (nounset)
    for arg in &argv[1..] {
        match arg.as_str() {
            "-e" => crate::env::set_var("ERREXIT", "1"),
            "+e" => crate::env::unset_var("ERREXIT"),
            "-x" => crate::env::set_var("XTRACE", "1"),
            "+x" => crate::env::unset_var("XTRACE"),
            "-u" => crate::env::set_var("NOUNSET", "1"),
            "+u" => crate::env::unset_var("NOUNSET"),
            _ => {}
        }
    }
    0
}

fn builtin_type(exec: &mut Executor, argv: &[String]) -> i32 {
    for name in &argv[1..] {
        if exec.aliases.contains_key(name.as_str()) {
            println!("{} is an alias for '{}'", name, exec.aliases[name.as_str()]);
        } else if exec.functions.contains_key(name.as_str()) {
            println!("{} is a shell function", name);
        } else if is_builtin(name) {
            println!("{} is a shell builtin", name);
        } else if let Some(path) = find_in_path(name) {
            println!("{} is {}", name, path);
        } else {
            eprintln!("{} not found", name);
            return 1;
        }
    }
    0
}

fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "pwd" | "echo" | "printf" | "export" | "unset" | "alias" |
        "unalias" | "source" | "." | "exit" | "true" | "false" | ":" | "set" |
        "return" | "break" | "continue" | "type" | "which" | "hash" | "jobs" |
        "fg" | "bg" | "history" | "test" | "[" | "read" | "eval" | "exec" |
        "umask" | "ulimit" | "wait" | "builtin" | "command")
}

fn find_in_path(name: &str) -> Option<String> {
    std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
        .map(|p| p.join(name))
        .find(|p| p.exists() && p.is_file())
        .map(|p| p.to_string_lossy().to_string())
}

fn builtin_test(argv: &[String]) -> i32 {
    // Minimal test/[ implementation
    let args: Vec<&str> = if argv[0] == "[" {
        // Strip trailing ]
        let end = argv.len().saturating_sub(1);
        argv[1..end].iter().map(|s| s.as_str()).collect()
    } else {
        argv[1..].iter().map(|s| s.as_str()).collect()
    };

    let result = match args.as_slice() {
        [s] => !s.is_empty(),
        ["-z", s] => s.is_empty(),
        ["-n", s] => !s.is_empty(),
        ["-e", path] => std::path::Path::new(path).exists(),
        ["-f", path] => std::path::Path::new(path).is_file(),
        ["-d", path] => std::path::Path::new(path).is_dir(),
        ["-r", path] => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path).map(|m| m.permissions().mode() & 0o444 != 0).unwrap_or(false)
        }
        ["-w", path] => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path).map(|m| m.permissions().mode() & 0o222 != 0).unwrap_or(false)
        }
        ["-x", path] => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
        }
        ["-s", path] => std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false),
        [a, "=", b] | [a, "==", b] => a == b,
        [a, "!=", b] => a != b,
        [a, "-eq", b] => {
            let (a, b): (i64, i64) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            a == b
        }
        [a, "-ne", b] => {
            let (a, b): (i64, i64) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            a != b
        }
        [a, "-lt", b] => {
            let (a, b): (i64, i64) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            a < b
        }
        [a, "-le", b] => {
            let (a, b): (i64, i64) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            a <= b
        }
        [a, "-gt", b] => {
            let (a, b): (i64, i64) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            a > b
        }
        [a, "-ge", b] => {
            let (a, b): (i64, i64) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            a >= b
        }
        ["-o", a, "-a", b] => {
            let r1 = builtin_test(&["test".to_string(), a.to_string()]) == 0;
            let r2 = builtin_test(&["test".to_string(), b.to_string()]) == 0;
            return if r1 || r2 { 0 } else { 1 };
        }
        ["!", rest @ ..] => {
            let mut test_argv = vec!["test".to_string()];
            test_argv.extend(rest.iter().map(|s| s.to_string()));
            return if builtin_test(&test_argv) == 0 { 1 } else { 0 };
        }
        _ => return 1,
    };

    if result { 0 } else { 1 }
}

fn builtin_read(argv: &[String]) -> Result<i32> {
    let var_name = argv.get(1).map(|s| s.as_str()).unwrap_or("REPLY");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let value = line.trim_end_matches('\n');
    crate::env::set_var(var_name, value);
    Ok(0)
}

fn builtin_eval(exec: &mut Executor, argv: &[String]) -> Result<i32> {
    let cmd = argv[1..].join(" ");
    exec.run_string(&cmd)
}

fn builtin_exec(argv: &[String]) -> Result<i32> {
    use std::ffi::CString;
    if argv.len() < 2 { return Ok(0); }
    let cmd = CString::new(argv[1].as_bytes())?;
    let args: Vec<CString> = argv[1..].iter()
        .map(|a| CString::new(a.as_bytes()).unwrap())
        .collect();
    nix::unistd::execvp(&cmd, &args)?;
    Ok(0)
}

fn builtin_umask(argv: &[String]) -> i32 {
    if argv.len() == 1 {
        let mask = unsafe { libc::umask(0) };
        unsafe { libc::umask(mask) };
        println!("{:04o}", mask);
    } else if let Ok(val) = u32::from_str_radix(&argv[1], 8) {
        unsafe { libc::umask(val as libc::mode_t) };
    }
    0
}

fn builtin_wait(exec: &mut Executor, argv: &[String]) -> Result<i32> {
    if argv.len() == 1 {
        // Wait for all background jobs
        Ok(0)
    } else {
        let pid: i32 = argv[1].parse().unwrap_or(0);
        let nix_pid = nix::unistd::Pid::from_raw(pid);
        exec.wait_child(nix_pid)
    }
}
