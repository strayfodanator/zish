use crate::config::Config;
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use reedline::{Completer, DefaultCompleter, Span, Suggestion};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Help cache: command → list of (flag, description)
static HELP_CACHE: OnceLock<std::sync::Mutex<HashMap<String, Vec<(String, String)>>>> =
    OnceLock::new();

fn help_cache() -> &'static std::sync::Mutex<HashMap<String, Vec<(String, String)>>> {
    HELP_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// zish completer — handles:
/// - commands (PATH lookup, builtins, aliases, functions)
/// - files/directories (case-insensitive)
/// - env variables ($VAR)
/// - command flags from --help output
/// - git subcommands
pub struct ZishCompleter {
    fuzzy: bool,
    case_sensitive: bool,
    parse_help: bool,
    matcher: SkimMatcherV2,
}

impl ZishCompleter {
    pub fn new(config: &Config) -> Self {
        Self {
            fuzzy: config.completion.fuzzy,
            case_sensitive: config.shell.case_sensitive_completion,
            parse_help: config.completion.parse_help,
            matcher: SkimMatcherV2::default(),
        }
    }

    /// Find all completions for a partial command name
    fn complete_command(&self, partial: &str) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();

        // Builtins
        let builtins = [
            "cd", "pwd", "echo", "printf", "export", "unset", "alias", "unalias",
            "source", "exit", "true", "false", "set", "return", "break", "continue",
            "type", "which", "jobs", "fg", "bg", "history", "test", "read", "eval",
            "exec", "wait", "command", "builtin",
        ];
        for b in &builtins {
            if self.matches(partial, b) {
                suggestions.push(Suggestion {
                    value: b.to_string(),
                    description: Some("builtin".to_string()),
                    style: None,
                    extra: None,
                    span: Span::new(0, partial.len()),
                    append_whitespace: true,
                });
            }
        }

        // PATH commands
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if self.matches(partial, &name) {
                            suggestions.push(Suggestion {
                                value: name,
                                description: None,
                                style: None,
                                extra: None,
                                span: Span::new(0, partial.len()),
                                append_whitespace: true,
                            });
                        }
                    }
                }
            }
        }

        // Deduplicate by value
        suggestions.sort_by(|a, b| a.value.cmp(&b.value));
        suggestions.dedup_by(|a, b| a.value == b.value);
        suggestions
    }

    /// Complete a file/directory path
    fn complete_path(&self, partial: &str) -> Vec<Suggestion> {
        let (dir_part, file_part) = split_path(partial);
        let search_dir = if dir_part.is_empty() { ".".to_string() } else { dir_part.clone() };

        let Ok(entries) = std::fs::read_dir(&search_dir) else { return vec![] };

        let mut suggestions = Vec::new();
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();

            // Case-insensitive or case-sensitive matching
            let matches = if self.case_sensitive {
                name.starts_with(&file_part)
            } else {
                name.to_lowercase().starts_with(&file_part.to_lowercase())
            };

            if matches {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let full = if dir_part.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", dir_part.trim_end_matches('/'), name)
                };
                let display = if is_dir { format!("{}/", full) } else { full.clone() };

                suggestions.push(Suggestion {
                    value: display,
                    description: if is_dir { Some("dir".to_string()) } else { None },
                    style: None,
                    extra: None,
                    span: Span::new(0, partial.len()),
                    append_whitespace: !is_dir,
                });
            }
        }

        suggestions.sort_by(|a, b| a.value.cmp(&b.value));
        suggestions
    }

    /// Complete $VARIABLE names
    fn complete_variable(&self, partial: &str) -> Vec<Suggestion> {
        // partial includes the leading $
        let var_part = partial.trim_start_matches('$');
        let env = crate::env::SHELL_ENV.read().unwrap();
        let vars: Vec<Suggestion> = env
            .exported_vars()
            .keys()
            .filter(|k| {
                if self.case_sensitive {
                    k.starts_with(var_part)
                } else {
                    k.to_lowercase().starts_with(&var_part.to_lowercase())
                }
            })
            .map(|k| Suggestion {
                value: format!("${}", k),
                description: Some(
                    env.get(k).unwrap_or_default().chars().take(30).collect::<String>()
                ),
                style: None,
                extra: None,
                span: Span::new(0, partial.len()),
                append_whitespace: true,
            })
            .collect();
        vars
    }

    /// Complete flags from --help output (cached)
    fn complete_flags(&self, cmd: &str, partial: &str) -> Vec<Suggestion> {
        let flags = get_or_parse_help(cmd);
        flags
            .iter()
            .filter(|(flag, _)| self.matches(partial, flag))
            .map(|(flag, desc)| Suggestion {
                value: flag.clone(),
                description: Some(desc.clone()),
                style: None,
                extra: None,
                span: Span::new(0, partial.len()),
                append_whitespace: true,
            })
            .collect()
    }

    fn matches(&self, partial: &str, candidate: &str) -> bool {
        if partial.is_empty() { return true; }
        if self.fuzzy {
            self.matcher.fuzzy_match(candidate, partial).is_some()
        } else if self.case_sensitive {
            candidate.starts_with(partial)
        } else {
            candidate.to_lowercase().starts_with(&partial.to_lowercase())
        }
    }
}

impl Completer for ZishCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let before_cursor = &line[..pos];
        let tokens: Vec<&str> = before_cursor.split_whitespace().collect();

        if tokens.is_empty() || (tokens.len() == 1 && !before_cursor.ends_with(' ')) {
            // Completing the command name
            let partial = tokens.first().copied().unwrap_or("");
            return self.complete_command(partial);
        }

        // We're completing an argument
        let cmd = tokens[0];
        let partial = if before_cursor.ends_with(' ') {
            ""
        } else {
            tokens.last().copied().unwrap_or("")
        };

        // $VAR completion
        if partial.starts_with('$') {
            return self.complete_variable(partial);
        }

        // Flag completion (-- or -)
        if partial.starts_with('-') && self.parse_help {
            let mut suggestions = self.complete_flags(cmd, partial);
            if !suggestions.is_empty() {
                return suggestions;
            }
        }

        // Git-specific subcommand completion
        if cmd == "git" && tokens.len() <= 2 && !before_cursor.ends_with(' ') {
            let partial = tokens.get(1).copied().unwrap_or("");
            return complete_git_subcommands(partial);
        }

        // Default: file/path completion
        self.complete_path(partial)
    }
}

// ─── Git subcommand completion ─────────────────────────────────────────────────

fn complete_git_subcommands(partial: &str) -> Vec<Suggestion> {
    let subcmds = [
        ("add", "Add file contents to the index"),
        ("commit", "Record changes to the repository"),
        ("push", "Update remote refs"),
        ("pull", "Fetch from and integrate with another repository"),
        ("fetch", "Download objects and refs from another repository"),
        ("status", "Show the working tree status"),
        ("log", "Show commit logs"),
        ("diff", "Show changes between commits"),
        ("branch", "List, create, or delete branches"),
        ("checkout", "Switch branches or restore working tree files"),
        ("switch", "Switch branches"),
        ("restore", "Restore working tree files"),
        ("merge", "Join two or more development histories together"),
        ("rebase", "Reapply commits on top of another base"),
        ("stash", "Stash the changes in a dirty working directory"),
        ("clone", "Clone a repository into a new directory"),
        ("init", "Create an empty Git repository"),
        ("remote", "Manage set of tracked repositories"),
        ("reset", "Reset current HEAD to the specified state"),
        ("tag", "Create, list, delete or verify a tag"),
        ("show", "Show various types of objects"),
        ("cherry-pick", "Apply the changes introduced by some existing commits"),
        ("bisect", "Use binary search to find the commit that introduced a bug"),
        ("blame", "Show what revision and author last modified each line"),
        ("shortlog", "Summarize git log output"),
        ("reflog", "Manage reflog information"),
    ];

    subcmds
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(partial))
        .map(|(cmd, desc)| Suggestion {
            value: cmd.to_string(),
            description: Some(desc.to_string()),
            style: None,
            extra: None,
            span: Span::new(0, partial.len()),
            append_whitespace: true,
        })
        .collect()
}

// ─── --help parser ─────────────────────────────────────────────────────────────

fn get_or_parse_help(cmd: &str) -> Vec<(String, String)> {
    let cache = help_cache();
    if let Ok(c) = cache.lock() {
        if let Some(flags) = c.get(cmd) {
            return flags.clone();
        }
    }

    // Parse help output
    let flags = parse_help_output(cmd);

    if let Ok(mut c) = cache.lock() {
        c.insert(cmd.to_string(), flags.clone());
    }
    flags
}

fn parse_help_output(cmd: &str) -> Vec<(String, String)> {
    // Try --help first, then -h
    let output = std::process::Command::new(cmd)
        .arg("--help")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output();

    let text = match output {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        }
        Err(_) => return vec![],
    };

    // Parse lines like: "  -f, --foo     Description"  or "  --bar=VALUE   Description"
    let mut flags = Vec::new();
    let re = regex::Regex::new(
        r"^\s{1,8}(-{1,2}[\w-]+(?:=[\w<>\[\]]+)?)(?:,\s+(-{1,2}[\w-]+(?:=[\w<>\[\]]+)?))?\s{2,}(.+)?$"
    ).unwrap();

    for line in text.lines() {
        if let Some(caps) = re.captures(line) {
            let short = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
            let long = caps.get(2).map(|m| m.as_str().to_string());
            let desc = caps.get(3).map(|m| m.as_str().trim().to_string()).unwrap_or_default();

            if !short.is_empty() {
                flags.push((short.clone(), desc.clone()));
            }
            if let Some(l) = long {
                flags.push((l, desc));
            }
        }
    }

    flags
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn split_path(partial: &str) -> (String, String) {
    if let Some(pos) = partial.rfind('/') {
        (partial[..=pos].to_string(), partial[pos + 1..].to_string())
    } else {
        (String::new(), partial.to_string())
    }
}
