use anyhow::Result;
use mlua::prelude::*;
use std::path::Path;

pub struct PluginEngine {
    lua: Lua,
}

impl PluginEngine {
    pub fn new() -> Result<Self> {
        let lua = Lua::new();
        Self::register_api(&lua)?;
        Ok(Self { lua })
    }

    fn register_api(lua: &Lua) -> Result<()> {
        let zish = lua.create_table()
            .map_err(|e| anyhow::anyhow!("lua: {}", e))?;

        let env_get = lua.create_function(|_, name: String| {
            Ok(crate::env::get_var(&name).unwrap_or_default())
        }).map_err(|e| anyhow::anyhow!("{}", e))?;

        let env_set = lua.create_function(|_, (name, value): (String, String)| {
            crate::env::set_var(name, value);
            Ok(())
        }).map_err(|e| anyhow::anyhow!("{}", e))?;

        let env_export = lua.create_function(|_, (name, value): (String, String)| {
            crate::env::export_var(name, Some(value));
            Ok(())
        }).map_err(|e| anyhow::anyhow!("{}", e))?;

        let env_table = lua.create_table()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        env_table.set("get", env_get).map_err(|e| anyhow::anyhow!("{}", e))?;
        env_table.set("set", env_set).map_err(|e| anyhow::anyhow!("{}", e))?;
        env_table.set("export", env_export).map_err(|e| anyhow::anyhow!("{}", e))?;
        zish.set("env", env_table).map_err(|e| anyhow::anyhow!("{}", e))?;

        let cwd_fn = lua.create_function(|_, ()| {
            Ok(crate::env::get_cwd())
        }).map_err(|e| anyhow::anyhow!("{}", e))?;
        zish.set("cwd", cwd_fn).map_err(|e| anyhow::anyhow!("{}", e))?;

        let run_fn = lua.create_function(|_, cmd: String| {
            let config = crate::config::Config::load()
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
            let mut exec = crate::executor::Executor::new(config);
            let code = exec.run_string(&cmd)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
            Ok(code)
        }).map_err(|e| anyhow::anyhow!("{}", e))?;
        zish.set("run", run_fn).map_err(|e| anyhow::anyhow!("{}", e))?;

        let capture_fn = lua.create_function(|_, cmd: String| {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .output()
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let code = output.status.code().unwrap_or(-1);
            Ok((stdout, code))
        }).map_err(|e| anyhow::anyhow!("{}", e))?;
        zish.set("capture", capture_fn).map_err(|e| anyhow::anyhow!("{}", e))?;

        let hist_add = lua.create_function(|_, cmd: String| {
            if let Ok(mut h) = crate::history::HISTORY.write() {
                let _ = h.add(&cmd, 0);
            }
            Ok(())
        }).map_err(|e| anyhow::anyhow!("{}", e))?;
        let hist_table = lua.create_table()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        hist_table.set("add", hist_add).map_err(|e| anyhow::anyhow!("{}", e))?;
        zish.set("history", hist_table).map_err(|e| anyhow::anyhow!("{}", e))?;

        // Hook tables
        let hooks_table = lua.create_table()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        hooks_table.set("precmd", lua.create_table().map_err(|e| anyhow::anyhow!("{}", e))?)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        hooks_table.set("preexec", lua.create_table().map_err(|e| anyhow::anyhow!("{}", e))?)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        hooks_table.set("on_exit", lua.create_table().map_err(|e| anyhow::anyhow!("{}", e))?)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        zish.set("hooks", hooks_table).map_err(|e| anyhow::anyhow!("{}", e))?;

        zish.set("version", env!("CARGO_PKG_VERSION"))
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        lua.globals().set("zish", zish)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(())
    }

    pub fn load_file(&self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path)?;
        self.lua
            .load(&content)
            .set_name(path.to_string_lossy().as_ref())
            .exec()
            .map_err(|e| anyhow::anyhow!("plugin {}: {}", path.display(), e))
    }

    pub fn load_all(&self) -> Result<()> {
        let plugin_dir = crate::config::Config::config_dir().join("plugins");
        if !plugin_dir.exists() { return Ok(()); }

        let mut paths: Vec<_> = std::fs::read_dir(&plugin_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "lua").unwrap_or(false))
            .map(|e| e.path())
            .collect();
        paths.sort();

        for path in paths {
            if let Err(e) = self.load_file(&path) {
                eprintln!("zish: plugin error: {}", e);
            }
        }
        Ok(())
    }

    pub fn run_precmd_hooks(&self) -> Result<()> {
        self.run_hooks("precmd")
    }

    pub fn run_preexec_hooks(&self, cmd: &str) -> Result<()> {
        let zish: LuaTable = self.lua.globals().get("zish")
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let hooks: LuaTable = zish.get::<LuaTable>("hooks")
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .get("preexec")
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        for pair in hooks.sequence_values::<LuaFunction>() {
            if let Ok(f) = pair {
                let _ = f.call::<()>(cmd.to_string());
            }
        }
        Ok(())
    }

    fn run_hooks(&self, name: &str) -> Result<()> {
        let zish: LuaTable = self.lua.globals().get("zish")
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let hooks: LuaTable = zish.get::<LuaTable>("hooks")
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .get(name)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        for pair in hooks.sequence_values::<LuaFunction>() {
            if let Ok(f) = pair {
                let _ = f.call::<()>(());
            }
        }
        Ok(())
    }

    pub fn eval(&self, code: &str) -> Result<String> {
        let result = self.lua.load(code).eval::<LuaValue>()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(match result {
            LuaValue::String(s) => s.to_str()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            LuaValue::Integer(i) => i.to_string(),
            LuaValue::Number(f)  => f.to_string(),
            LuaValue::Boolean(b) => b.to_string(),
            LuaValue::Nil        => String::new(),
            other                => format!("{:?}", other),
        })
    }
}
