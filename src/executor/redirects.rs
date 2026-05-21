use crate::parser::ast::{Redirect, RedirectKind, RedirectTarget};
use anyhow::{bail, Result};
use std::fs::{File, OpenOptions};
use std::os::unix::io::{FromRawFd, IntoRawFd, RawFd};

pub fn apply_redirects(redirects: &[Redirect]) -> Result<()> {
    for redir in redirects {
        apply_one(redir)?;
    }
    Ok(())
}

fn apply_one(redir: &Redirect) -> Result<()> {
    let target_fd = redir.fd;

    match &redir.kind {
        RedirectKind::Output => {
            let path = word_to_string(&redir.target)?;
            let file = File::create(&path)?;
            nix::unistd::dup2(file.into_raw_fd(), target_fd)?;
        }
        RedirectKind::Append => {
            let path = word_to_string(&redir.target)?;
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            nix::unistd::dup2(file.into_raw_fd(), target_fd)?;
        }
        RedirectKind::Input => {
            let path = word_to_string(&redir.target)?;
            let file = File::open(&path)?;
            nix::unistd::dup2(file.into_raw_fd(), 0)?;
        }
        RedirectKind::RedirectBoth => {
            let path = word_to_string(&redir.target)?;
            let file = File::create(&path)?;
            let fd = file.into_raw_fd();
            nix::unistd::dup2(fd, 1)?;
            nix::unistd::dup2(fd, 2)?;
        }
        RedirectKind::AppendBoth => {
            let path = word_to_string(&redir.target)?;
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            let fd = file.into_raw_fd();
            nix::unistd::dup2(fd, 1)?;
            nix::unistd::dup2(fd, 2)?;
        }
        RedirectKind::DupFd => {
            if let RedirectTarget::Fd(src_fd) = &redir.target {
                nix::unistd::dup2(*src_fd, target_fd)?;
            }
        }
        RedirectKind::HereString => {
            let value = word_to_string(&redir.target)?;
            let (read_raw, write_raw) = make_pipe()?;
            match unsafe { nix::unistd::fork() }? {
                nix::unistd::ForkResult::Child => {
                    unsafe { nix::unistd::close(read_raw).ok() };
                    {
                        let mut file = unsafe { File::from_raw_fd(write_raw) };
                        use std::io::Write;
                        let _ = file.write_all(value.as_bytes());
                        let _ = file.write_all(b"\n");
                    }
                    std::process::exit(0);
                }
                nix::unistd::ForkResult::Parent { .. } => {
                    unsafe { nix::unistd::close(write_raw).ok() };
                    nix::unistd::dup2(read_raw, 0)?;
                    unsafe { nix::unistd::close(read_raw).ok() };
                }
            }
        }
        RedirectKind::HereDoc => {
            let value = word_to_string(&redir.target)?;
            let (read_raw, write_raw) = make_pipe()?;
            match unsafe { nix::unistd::fork() }? {
                nix::unistd::ForkResult::Child => {
                    unsafe { nix::unistd::close(read_raw).ok() };
                    {
                        let mut file = unsafe { File::from_raw_fd(write_raw) };
                        use std::io::Write;
                        let _ = file.write_all(value.as_bytes());
                    }
                    std::process::exit(0);
                }
                nix::unistd::ForkResult::Parent { .. } => {
                    unsafe { nix::unistd::close(write_raw).ok() };
                    nix::unistd::dup2(read_raw, 0)?;
                    unsafe { nix::unistd::close(read_raw).ok() };
                }
            }
        }
    }
    Ok(())
}

/// Create a pipe and return (read_fd, write_fd) as raw RawFd integers.
fn make_pipe() -> Result<(RawFd, RawFd)> {
    use std::os::unix::io::IntoRawFd;
    let (r, w) = nix::unistd::pipe()?;
    Ok((r.into_raw_fd(), w.into_raw_fd()))
}

fn word_to_string(target: &RedirectTarget) -> Result<String> {
    match target {
        RedirectTarget::File(w) => {
            use crate::scripting::expand::Expander;
            Ok(Expander::new().expand_word(w)?)
        }
        RedirectTarget::Fd(_) => bail!("expected file target"),
    }
}
