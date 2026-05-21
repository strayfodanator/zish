use anyhow::Result;
use once_cell::sync::Lazy;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::RwLock;

pub struct History {
    entries: Vec<String>,
    db_path: PathBuf,
    max_size: usize,
}

impl History {
    pub fn new(db_path: PathBuf, max_size: usize) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                cmd     TEXT NOT NULL,
                ts      INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                cwd     TEXT,
                exit    INTEGER
            );
            CREATE INDEX IF NOT EXISTS history_ts ON history(ts);",
        )?;

        let mut stmt = conn.prepare(
            "SELECT cmd FROM history ORDER BY id DESC LIMIT ?1",
        )?;
        let entries: Vec<String> = stmt
            .query_map(params![max_size as i64], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        Ok(Self { entries, db_path, max_size })
    }

    pub fn add(&mut self, cmd: &str, exit_code: i32) -> Result<()> {
        let cmd = cmd.trim();
        if cmd.is_empty() { return Ok(()); }
        if self.entries.last().map(|s| s.as_str()) == Some(cmd) {
            return Ok(());
        }

        self.entries.push(cmd.to_string());
        if self.entries.len() > self.max_size {
            self.entries.remove(0);
        }

        let db_path = self.db_path.clone();
        let cmd_owned = cmd.to_string();
        let cwd = crate::env::get_cwd();
        std::thread::spawn(move || {
            if let Ok(conn) = Connection::open(&db_path) {
                let _ = conn.execute(
                    "INSERT INTO history (cmd, cwd, exit) VALUES (?1, ?2, ?3)",
                    params![cmd_owned, cwd, exit_code],
                );
                let _ = conn.execute(
                    "DELETE FROM history WHERE id NOT IN (
                        SELECT id FROM history ORDER BY id DESC LIMIT 100000
                    )",
                    [],
                );
            }
        });

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn last_n(&self, n: usize) -> Vec<String> {
        let start = self.entries.len().saturating_sub(n);
        self.entries[start..].to_vec()
    }

    pub fn search(&self, query: &str) -> Vec<String> {
        if query.is_empty() {
            return self.entries.iter().rev().cloned().collect();
        }
        self.entries
            .iter()
            .rev()
            .filter(|e| e.contains(query))
            .cloned()
            .collect()
    }

    pub fn all_entries(&self) -> &[String] {
        &self.entries
    }

    /// Build a reedline FileBackedHistory (in-memory) from our entries
    pub fn to_reedline_history(&self) -> reedline::FileBackedHistory {
        let mut hist = reedline::FileBackedHistory::new(self.max_size)
            .expect("failed to create reedline history");
        for entry in &self.entries {
            use reedline::History;
            let _ = hist.save(reedline::HistoryItem {
                id: None,
                start_timestamp: None,
                command_line: entry.clone(),
                session_id: None,
                hostname: None,
                cwd: None,
                duration: None,
                exit_status: None,
                more_info: None,
            });
        }
        hist
    }
}

fn default_history_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("zish")
        .join("history.db")
}

pub static HISTORY: Lazy<RwLock<History>> = Lazy::new(|| {
    let path = crate::env::get_var("ZISH_HISTORY_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(default_history_path);
    let size = crate::env::get_var("ZISH_HISTORY_SIZE")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);

    RwLock::new(History::new(path, size).unwrap_or_else(|_| History {
        entries: Vec::new(),
        db_path: default_history_path(),
        max_size: size,
    }))
});
