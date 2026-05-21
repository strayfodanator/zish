pub mod expand;

use crate::executor::Executor;
use crate::parser::ast::Statement;
use anyhow::Result;

/// Execute a `.zh` or `.sh` script
pub fn run_script(exec: &mut Executor, path: &std::path::Path, args: &[String]) -> Result<i32> {
    exec.run_file(path, args)
}
