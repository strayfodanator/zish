use crate::executor::Executor;
use anyhow::Result;

pub fn builtin_history(exec: &mut Executor, argv: &[String]) -> Result<i32> {
    let n: usize = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(50);

    let history = crate::history::HISTORY.read().unwrap();
    let entries = history.last_n(n);
    let total = history.len();

    for (i, entry) in entries.iter().enumerate() {
        println!("{:5}  {}", total - entries.len() + i + 1, entry);
    }
    Ok(0)
}
