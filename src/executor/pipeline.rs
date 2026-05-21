use crate::executor::Executor;
use crate::parser::ast::Statement;
use anyhow::Result;
use nix::unistd::{fork, ForkResult};
use std::os::unix::io::{IntoRawFd, RawFd};

pub fn execute_pipeline(
    exec: &mut Executor,
    commands: &[Statement],
    stderr_pipes: &[bool],
) -> Result<i32> {
    if commands.len() == 1 {
        return exec.execute(&commands[0]);
    }

    let n = commands.len();
    // Create n-1 pipes: each returns (read_raw, write_raw) as RawFd
    let mut pipes: Vec<(RawFd, RawFd)> = Vec::new();
    for _ in 0..n - 1 {
        let (r, w) = nix::unistd::pipe()?;
        pipes.push((r.into_raw_fd(), w.into_raw_fd()));
    }

    let mut child_pids = Vec::new();

    for (i, cmd) in commands.iter().enumerate() {
        match unsafe { fork() }? {
            ForkResult::Child => {
                // Connect stdin from previous pipe read end
                if i > 0 {
                    nix::unistd::dup2(pipes[i - 1].0, 0).ok();
                }
                // Connect stdout to next pipe write end
                if i < n - 1 {
                    nix::unistd::dup2(pipes[i].1, 1).ok();
                    if stderr_pipes.get(i).copied().unwrap_or(false) {
                        nix::unistd::dup2(pipes[i].1, 2).ok();
                    }
                }
                // Close all pipe fds in child
                for &(r, w) in &pipes {
                    unsafe {
                        nix::unistd::close(r).ok();
                        nix::unistd::close(w).ok();
                    }
                }
                let code = exec.execute(cmd).unwrap_or(1);
                std::process::exit(code);
            }
            ForkResult::Parent { child } => {
                child_pids.push(child);
            }
        }
    }

    // Parent: close all pipe fds
    for &(r, w) in &pipes {
        unsafe {
            nix::unistd::close(r).ok();
            nix::unistd::close(w).ok();
        }
    }

    // Wait for all children; return exit code of last
    let mut last_code = 0;
    for pid in child_pids {
        last_code = exec.wait_child(pid)?;
    }

    Ok(last_code)
}
