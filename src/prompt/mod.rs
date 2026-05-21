use anyhow::Result;
use std::process::Command;

/// Prompt rendering — supports "starship" and "builtin" engines
pub struct PromptRenderer {
    engine: PromptEngine,
}

enum PromptEngine {
    Starship,
    Builtin { left: String, right: String },
}

impl PromptRenderer {
    pub fn new(config: &crate::config::Config) -> Self {
        let engine = if config.prompt.engine == "starship" && starship_available() {
            PromptEngine::Starship
        } else {
            PromptEngine::Builtin {
                left: config.prompt.left.clone(),
                right: config.prompt.right.clone(),
            }
        };
        Self { engine }
    }

    /// Render the left prompt string
    pub fn render_left(&self, last_exit: i32, cmd_duration_ms: u64, jobs_count: usize) -> String {
        match &self.engine {
            PromptEngine::Starship => {
                render_starship_prompt(last_exit, cmd_duration_ms, jobs_count)
                    .unwrap_or_else(|_| default_prompt())
            }
            PromptEngine::Builtin { left, .. } => {
                render_builtin(left, last_exit)
            }
        }
    }

    /// Render the right prompt string (empty if not configured)
    pub fn render_right(&self, last_exit: i32, cmd_duration_ms: u64, jobs_count: usize) -> String {
        match &self.engine {
            PromptEngine::Starship => {
                render_starship_right_prompt(last_exit, cmd_duration_ms, jobs_count)
                    .unwrap_or_default()
            }
            PromptEngine::Builtin { right, .. } => {
                render_builtin(right, last_exit)
            }
        }
    }
}

fn starship_available() -> bool {
    which::which("starship").is_ok()
}

/// Call `starship prompt` with the right environment variables
fn render_starship_prompt(exit_code: i32, cmd_duration_ms: u64, jobs: usize) -> Result<String> {
    // Starship reads these from the environment
    std::env::set_var("STARSHIP_SHELL", "zish");

    let output = Command::new("starship")
        .arg("prompt")
        .arg("--terminal-width")
        .arg(terminal_width().to_string())
        .arg("--status")
        .arg(exit_code.to_string())
        .arg("--cmd-duration")
        .arg(cmd_duration_ms.to_string())
        .arg("--jobs")
        .arg(jobs.to_string())
        .env("STARSHIP_SHELL", "zish")
        .env("PWD", crate::env::get_cwd())
        .output()?;

    let prompt = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(prompt)
}

fn render_starship_right_prompt(exit_code: i32, cmd_duration_ms: u64, jobs: usize) -> Result<String> {
    let output = Command::new("starship")
        .arg("prompt")
        .arg("--right")
        .arg("--terminal-width")
        .arg(terminal_width().to_string())
        .arg("--status")
        .arg(exit_code.to_string())
        .arg("--cmd-duration")
        .arg(cmd_duration_ms.to_string())
        .env("STARSHIP_SHELL", "zish")
        .env("PWD", crate::env::get_cwd())
        .output()?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Built-in prompt format: {cwd}, {user}, {host}, {git_branch}, {exit_code}, {time}
fn render_builtin(template: &str, exit_code: i32) -> String {
    let cwd = crate::env::get_cwd();
    let cwd = abbreviate_home(&cwd);
    let user = crate::env::get_var("USER").unwrap_or_else(|| "user".to_string());
    let host = hostname();
    let git = git_branch().unwrap_or_default();
    let time = current_time();
    let exit_indicator = if exit_code != 0 {
        format!("\x1b[31m✗{}\x1b[0m ", exit_code)
    } else {
        String::new()
    };

    template
        .replace("{cwd}", &cwd)
        .replace("{user}", &user)
        .replace("{host}", &host)
        .replace("{git_branch}", &if git.is_empty() { String::new() } else { format!(" \x1b[35m{}\x1b[0m", git) })
        .replace("{exit_code}", &exit_indicator)
        .replace("{time}", &time)
        .replace("{newline}", "\n")
}

fn default_prompt() -> String {
    let cwd = abbreviate_home(&crate::env::get_cwd());
    format!("\x1b[1;34m{}\x1b[0m \x1b[1;32m❯\x1b[0m ", cwd)
}

fn abbreviate_home(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path.starts_with(home_str.as_ref()) {
            return path.replacen(home_str.as_ref(), "~", 1);
        }
    }
    path.to_string()
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn git_branch() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Some(format!(" {}", branch))
    } else {
        None
    }
}

fn current_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

fn terminal_width() -> u16 {
    crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80)
}
