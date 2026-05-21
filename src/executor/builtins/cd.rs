use crate::executor::Executor;
use crate::parser::ast::Redirect;
use anyhow::Result;

pub fn builtin_cd(exec: &mut Executor, argv: &[String], _redirects: &[Redirect]) -> Result<i32> {
    let target = match argv.get(1) {
        None => {
            // cd with no args → go home
            dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"))
        }
        Some(s) if s == "-" => {
            // cd - → go to OLDPWD
            let old = crate::env::get_var("OLDPWD").unwrap_or_else(|| "/".to_string());
            println!("{}", old);
            std::path::PathBuf::from(old)
        }
        Some(s) if s.starts_with("~") => {
            let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
            if s == "~" {
                home
            } else {
                home.join(&s[2..])
            }
        }
        Some(s) => std::path::PathBuf::from(s),
    };

    // Resolve symlinks for CDPATH support
    let target = if target.is_absolute() {
        target
    } else {
        // Check CDPATH
        let cdpath = crate::env::get_var("CDPATH").unwrap_or_default();
        let found = std::env::split_paths(&cdpath)
            .map(|p| p.join(&target))
            .find(|p| p.is_dir());
        found.unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                .join(&target)
        })
    };

    // Canonicalize path (resolve .., ., symlinks)
    let canonical = target.canonicalize().map_err(|e| {
        anyhow::anyhow!("cd: {}: {}", target.display(), e)
    })?;

    std::env::set_current_dir(&canonical).map_err(|e| {
        anyhow::anyhow!("cd: {}: {}", canonical.display(), e)
    })?;

    crate::env::SHELL_ENV.write().unwrap().set_cwd(&canonical);
    Ok(0)
}
