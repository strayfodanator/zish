use std::collections::HashMap;
use std::env;
use std::sync::{Arc, RwLock};

/// Shell variable scope
#[derive(Debug, Clone, PartialEq)]
pub enum VarScope {
    Local,
    Exported,
}

#[derive(Debug, Clone)]
pub struct ShellVar {
    pub value: String,
    pub scope: VarScope,
    pub readonly: bool,
}

/// Global shell environment state
pub struct ShellEnv {
    vars: HashMap<String, ShellVar>,
}

impl ShellEnv {
    pub fn new() -> Self {
        let mut vars = HashMap::new();

        // Import current process environment
        for (key, value) in env::vars() {
            vars.insert(
                key,
                ShellVar {
                    value,
                    scope: VarScope::Exported,
                    readonly: false,
                },
            );
        }

        // Set shell-specific vars
        let pid = std::process::id().to_string();
        vars.insert(
            "ZISH_VERSION".to_string(),
            ShellVar {
                value: env!("CARGO_PKG_VERSION").to_string(),
                scope: VarScope::Exported,
                readonly: true,
            },
        );
        vars.insert(
            "SHELL".to_string(),
            ShellVar {
                value: std::env::current_exe()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "zish".to_string()),
                scope: VarScope::Exported,
                readonly: false,
            },
        );
        vars.insert(
            "$".to_string(),
            ShellVar {
                value: pid,
                scope: VarScope::Local,
                readonly: true,
            },
        );

        // Starship integration env vars
        vars.insert(
            "STARSHIP_SHELL".to_string(),
            ShellVar {
                value: "zish".to_string(),
                scope: VarScope::Exported,
                readonly: false,
            },
        );

        let session_key: u64 = rand::random();
        vars.insert(
            "STARSHIP_SESSION_KEY".to_string(),
            ShellVar {
                value: session_key.to_string(),
                scope: VarScope::Exported,
                readonly: false,
            },
        );

        Self { vars }
    }

    /// Get a variable's value, checking shell vars first then process env
    pub fn get(&self, name: &str) -> Option<String> {
        self.vars.get(name).map(|v| v.value.clone())
    }

    /// Set a variable (local by default)
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        let entry = self.vars.entry(name.clone()).or_insert(ShellVar {
            value: String::new(),
            scope: VarScope::Local,
            readonly: false,
        });
        if !entry.readonly {
            entry.value = value.clone();
            // If exported, also update process env
            if entry.scope == VarScope::Exported {
                env::set_var(&name, &value);
            }
        }
    }

    /// Export a variable (makes it available to child processes)
    pub fn export(&mut self, name: impl Into<String>, value: Option<String>) {
        let name = name.into();
        if let Some(val) = value.or_else(|| self.get(&name)) {
            env::set_var(&name, &val);
            let entry = self.vars.entry(name).or_insert(ShellVar {
                value: String::new(),
                scope: VarScope::Exported,
                readonly: false,
            });
            entry.value = val;
            entry.scope = VarScope::Exported;
        }
    }

    /// Unset a variable
    pub fn unset(&mut self, name: &str) {
        if let Some(v) = self.vars.get(name) {
            if !v.readonly {
                env::remove_var(name);
                self.vars.remove(name);
            }
        }
    }

    /// Mark a variable as readonly
    pub fn set_readonly(&mut self, name: &str) {
        if let Some(v) = self.vars.get_mut(name) {
            v.readonly = true;
        }
    }

    /// Get all exported variables (for child process env)
    pub fn exported_vars(&self) -> HashMap<String, String> {
        self.vars
            .iter()
            .filter(|(_, v)| v.scope == VarScope::Exported)
            .map(|(k, v)| (k.clone(), v.value.clone()))
            .collect()
    }

    /// Get current working directory
    pub fn cwd(&self) -> String {
        self.get("PWD")
            .unwrap_or_else(|| std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/".to_string()))
    }

    /// Update PWD after cd
    pub fn set_cwd(&mut self, path: &std::path::Path) {
        let path_str = path.to_string_lossy().to_string();
        if let Some(old) = self.get("PWD") {
            self.set("OLDPWD", old);
        }
        self.export("PWD", Some(path_str));
    }
}

// Global shared environment instance
use once_cell::sync::Lazy;
pub static SHELL_ENV: Lazy<Arc<RwLock<ShellEnv>>> =
    Lazy::new(|| Arc::new(RwLock::new(ShellEnv::new())));

/// Initialize the shell environment
pub fn init() {
    // Touch the lazy to initialize it
    let _ = SHELL_ENV.read().unwrap().get("ZISH_VERSION");
}

pub fn get_var(name: &str) -> Option<String> {
    SHELL_ENV.read().unwrap().get(name)
}

pub fn set_var(name: impl Into<String>, value: impl Into<String>) {
    SHELL_ENV.write().unwrap().set(name, value);
}

pub fn export_var(name: impl Into<String>, value: Option<String>) {
    SHELL_ENV.write().unwrap().export(name, value);
}

pub fn unset_var(name: &str) {
    SHELL_ENV.write().unwrap().unset(name);
}

pub fn get_cwd() -> String {
    SHELL_ENV.read().unwrap().cwd()
}
