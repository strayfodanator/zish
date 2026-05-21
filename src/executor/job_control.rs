use nix::unistd::Pid;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Job {
    pub pid: Pid,
    pub status: JobStatus,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Running,
    Stopped,
    Done(i32),
}

pub struct JobTable {
    jobs: HashMap<usize, Job>,
    next_id: usize,
}

impl JobTable {
    pub fn new() -> Self {
        Self { jobs: HashMap::new(), next_id: 1 }
    }

    pub fn add_background(&mut self, pid: Pid) -> usize {
        let id = self.next_id;
        self.jobs.insert(id, Job {
            pid,
            status: JobStatus::Running,
            command: String::new(), // filled in by caller
        });
        self.next_id += 1;
        id
    }

    pub fn add_stopped(&mut self, pid: Pid) -> usize {
        let id = self.next_id;
        self.jobs.insert(id, Job {
            pid,
            status: JobStatus::Stopped,
            command: String::new(),
        });
        self.next_id += 1;
        id
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn list(&self) -> Vec<(usize, &Job)> {
        let mut v: Vec<_> = self.jobs.iter().map(|(id, j)| (*id, j)).collect();
        v.sort_by_key(|(id, _)| *id);
        v
    }

    pub fn remove(&mut self, id: usize) {
        self.jobs.remove(&id);
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut Job> {
        self.jobs.get_mut(&id)
    }

    /// Reap any completed background jobs (non-blocking)
    pub fn reap(&mut self) {
        use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
        let ids: Vec<usize> = self.jobs.keys().copied().collect();
        for id in ids {
            if let Some(job) = self.jobs.get_mut(&id) {
                if job.status == JobStatus::Running {
                    if let Ok(WaitStatus::Exited(_, code)) =
                        waitpid(job.pid, Some(WaitPidFlag::WNOHANG))
                    {
                        job.status = JobStatus::Done(code);
                        println!("\n[{}] Done\t{}", id, job.command);
                    }
                }
            }
        }
        // Remove done jobs
        self.jobs.retain(|_, j| j.status != JobStatus::Done(0));
    }
}
