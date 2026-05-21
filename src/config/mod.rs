use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level configuration loaded from ~/.config/zish/zish.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub shell: ShellConfig,
    pub prompt: PromptConfig,
    pub keybindings: KeybindingsConfig,
    pub colors: ColorsConfig,
    pub aliases: HashMap<String, String>,
    pub completion: CompletionConfig,
    pub hooks: HooksConfig,
    pub history: HistoryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub auto_cd: bool,
    pub correct_typos: bool,
    pub case_sensitive_completion: bool,
    /// Characters treated as word boundaries
    pub word_chars: String,
    /// Max depth for recursive glob
    pub glob_max_depth: usize,
    /// Show exit code in prompt on failure
    pub show_exit_code: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptConfig {
    /// "starship" | "builtin"
    pub engine: String,
    /// Left prompt format (builtin engine only)
    pub left: String,
    /// Right prompt format (builtin engine only)
    pub right: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    /// "emacs" | "vi"
    pub mode: String,
    /// Custom keybindings: key -> action
    pub bindings: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    pub command: String,
    pub invalid_command: String,
    pub argument: String,
    pub string: String,
    pub comment: String,
    pub operator: String,
    pub number: String,
    pub path: String,
    pub variable: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletionConfig {
    /// "inline" | "list" | "menu"
    pub menu_style: String,
    pub fuzzy: bool,
    pub show_descriptions: bool,
    /// Parse --help output for flag completions
    pub parse_help: bool,
    pub path_separator: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Run before each command executes (receives the command string)
    pub preexec: Vec<String>,
    /// Run after each command, before prompt is displayed
    pub precmd: Vec<String>,
    /// Run on shell startup
    pub on_startup: Vec<String>,
    /// Run on shell exit
    pub on_exit: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub file: Option<PathBuf>,
    pub size: usize,
    pub dedup: bool,
    /// Share history across sessions in real time
    pub share: bool,
}

// ─── Default Implementations ──────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: ShellConfig::default(),
            prompt: PromptConfig::default(),
            keybindings: KeybindingsConfig::default(),
            colors: ColorsConfig::default(),
            aliases: default_aliases(),
            completion: CompletionConfig::default(),
            hooks: HooksConfig::default(),
            history: HistoryConfig::default(),
        }
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            auto_cd: true,
            correct_typos: true,
            case_sensitive_completion: false,
            word_chars: "*/?.[]~={},".to_string(),
            glob_max_depth: 16,
            show_exit_code: true,
        }
    }
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            engine: "starship".to_string(),
            left: "\x1b[38;2;129;140;248m{cwd}\x1b[0m{git_branch} \x1b[38;2;52;211;153m❯\x1b[0m ".to_string(),
            right: String::new(),
        }
    }
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            mode: "emacs".to_string(),
            bindings: HashMap::new(),
        }
    }
}

impl Default for ColorsConfig {
    fn default() -> Self {
        Self {
            command: "bold #38bdf8".to_string(),        // Sleek electric cyan
            invalid_command: "bold #f87171".to_string(),// Warm pastel red
            argument: "#e2e8f0".to_string(),           // Premium soft white
            string: "#34d399".to_string(),             // Mint emerald green
            comment: "italic #64748b".to_string(),     // Slate gray
            operator: "#c084fc".to_string(),           // Pastel violet
            number: "#fbbf24".to_string(),             // Soft amber yellow
            path: "bold #818cf8".to_string(),          // Royal indigo
            variable: "bold #f472b6".to_string(),      // Warm rose pink
        }
    }
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            menu_style: "menu".to_string(),
            fuzzy: true,
            show_descriptions: true,
            parse_help: true,
            path_separator: "/".to_string(),
        }
    }
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            preexec: vec![],
            precmd: vec![],
            on_startup: vec!["source ~/.config/zish/init.zh".to_string()],
            on_exit: vec![],
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            file: None,
            size: 100_000,
            dedup: true,
            share: true,
        }
    }
}

fn default_aliases() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("ll".to_string(), "ls -la --color=auto".to_string());
    m.insert("la".to_string(), "ls -A --color=auto".to_string());
    m.insert("l".to_string(), "ls -CF --color=auto".to_string());
    m.insert("..".to_string(), "cd ..".to_string());
    m.insert("...".to_string(), "cd ../..".to_string());
    m.insert("g".to_string(), "git".to_string());
    m
}

// ─── Config Loading ────────────────────────────────────────────────────────────

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path();

        if !config_path.exists() {
            // Write a default config on first run
            let default = Config::default();
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let toml_str = toml::to_string_pretty(&default)?;
            std::fs::write(&config_path, toml_str)?;
            return Ok(default);
        }

        let content = std::fs::read_to_string(&config_path)?;
        let config: Config = toml::from_str(&content).unwrap_or_else(|e| {
            eprintln!("zish: config parse error: {e}. Using defaults.");
            Config::default()
        });

        Ok(config)
    }

    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("zish")
            .join("zish.toml")
    }

    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("zish")
    }
}
