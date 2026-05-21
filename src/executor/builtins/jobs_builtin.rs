use crate::executor::Executor;
use anyhow::Result;

pub fn builtin_jobs(exec: &mut Executor, _argv: &[String]) -> Result<i32> {
    let jobs = exec.jobs.read().unwrap();
    for (id, job) in jobs.list() {
        let status = match &job.status {
            crate::executor::job_control::JobStatus::Running => "Running",
            crate::executor::job_control::JobStatus::Stopped => "Stopped",
            crate::executor::job_control::JobStatus::Done(_) => "Done",
        };
        println!("[{}] {:?}\t{}\t{}", id, job.pid, status, job.command);
    }
    Ok(0)
}

pub fn builtin_fg(exec: &mut Executor, argv: &[String]) -> Result<i32> {
    let id: usize = argv.get(1).and_then(|s| s.trim_start_matches('%').parse().ok()).unwrap_or(1);
    let pid = {
        let mut jobs = exec.jobs.write().unwrap();
        if let Some(job) = jobs.get_mut(id) {
            let pid = job.pid;
            job.status = crate::executor::job_control::JobStatus::Running;
            // Send SIGCONT
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGCONT);
            pid
        } else {
            anyhow::bail!("fg: {}: no such job", id);
        }
    };
    exec.wait_child(pid)
}

pub fn builtin_bg(exec: &mut Executor, argv: &[String]) -> Result<i32> {
    let id: usize = argv.get(1).and_then(|s| s.trim_start_matches('%').parse().ok()).unwrap_or(1);
    let mut jobs = exec.jobs.write().unwrap();
    if let Some(job) = jobs.get_mut(id) {
        job.status = crate::executor::job_control::JobStatus::Running;
        let _ = nix::sys::signal::kill(job.pid, nix::sys::signal::Signal::SIGCONT);
        println!("[{}] {}", id, job.command);
        Ok(0)
    } else {
        anyhow::bail!("bg: {}: no such job", id);
    }
}
