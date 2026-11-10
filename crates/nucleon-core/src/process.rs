//! Ownership of a launcher and its Unix process group.
use anyhow::{Context, Result};
use std::{
    process::{Child, Command, ExitStatus},
    thread,
    time::Duration,
};

pub const TERMINATION_GRACE: Duration = Duration::from_millis(300);

pub struct OwnedChild {
    child: Child,
    reaped: bool,
}

impl OwnedChild {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        Ok(Self {
            child: command.spawn()?,
            reaped: false,
        })
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn pid(&self) -> rustix::process::Pid {
        let raw = i32::try_from(self.child.id()).expect("Child PID fits Unix pid_t");
        assert!(
            raw > 1,
            "Owned launcher must not identify the init process group"
        );
        rustix::process::Pid::from_raw(raw).expect("Child PID is nonzero")
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        if self.reaped {
            return Ok(self.child.try_wait()?);
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            use rustix::process::{WaitId, WaitIdOptions, waitid};
            use std::os::unix::process::ExitStatusExt;
            // Observe without reaping: even an exited launcher pins its PID/PGID until group
            // cleanup, preventing a recycled group ID from identifying an unrelated process.
            let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
            let status = loop {
                match waitid(WaitId::Pid(self.pid()), options) {
                    Err(rustix::io::Errno::INTR) => continue,
                    result => break result.context("Could not observe owned launcher"),
                }
            }?;
            Ok(status.map(|status| {
                let raw = if let Some(code) = status.exit_status() {
                    code << 8
                } else {
                    status.terminating_signal().unwrap_or(0)
                        | if status.dumped() { 0x80 } else { 0 }
                };
                ExitStatus::from_raw(raw)
            }))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Ok(self.child.try_wait()?)
        }
    }

    pub fn terminate(&mut self, grace: Duration) -> Result<ExitStatus> {
        if self.reaped {
            return self.child.wait().context("Could not read launcher status");
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            use rustix::process::{Signal, kill_process_group};
            // An externally reaped child no longer proves ownership of this numeric PGID.
            self.try_wait()?;
            let group = self.pid();
            let signal = |signal| match kill_process_group(group, signal) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
                // Darwin reports EPERM when only zombies remain in a group. Do not hide
                // actual permission failures: confirm that there are no live members first.
                #[cfg(target_os = "macos")]
                Err(rustix::io::Errno::PERM) if !group_has_live_members(group) => Ok(()),
                Err(error) => Err(error).with_context(|| {
                    format!("Could not signal owned launcher group with {signal:?}")
                }),
            };
            let term = signal(Signal::TERM);
            thread::sleep(grace);
            let kill = signal(Signal::KILL);
            // Also stop the direct child if it moved to another group/session.
            let _ = self.child.kill();
            let status = self.child.wait().context("Could not reap owned launcher")?;
            self.reaped = true;
            term?;
            kill?;
            Ok(status)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = grace;
            self.child.kill().context("Could not stop owned launcher")?;
            let status = self.child.wait().context("Could not reap owned launcher")?;
            self.reaped = true;
            Ok(status)
        }
    }
}

#[cfg(target_os = "macos")]
fn group_has_live_members(group: rustix::process::Pid) -> bool {
    use sysinfo::{ProcessStatus, ProcessesToUpdate, System};
    let mut system = match System::new() {
        Ok(system) => system,
        Err(error) => {
            log::warn!("Cannot inspect launcher group: {error}");
            return true;
        }
    };
    system.refresh_processes(ProcessesToUpdate::All, true);
    system.processes().iter().any(|(pid, process)| {
        if process.status() == ProcessStatus::Zombie {
            return false;
        }
        let Some(pid) = i32::try_from(pid.as_u32())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        else {
            return true;
        };
        match rustix::process::getpgid(Some(pid)) {
            Ok(actual) => actual == group,
            Err(rustix::io::Errno::SRCH) => false,
            Err(_) => true, // Insufficient visibility cannot prove that cleanup completed.
        }
    })
}

#[derive(Debug)]
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
    grace: Duration,
) -> Result<ExitStatus> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child =
        OwnedChild::spawn(command).with_context(|| format!("Could not start {program}"))?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            child.terminate(Duration::ZERO)?;
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            let _ = child.terminate(grace);
            anyhow::bail!(
                "Command {program} timed out after {} ms",
                timeout.as_millis()
            );
        }
        thread::sleep(Duration::from_millis(15));
    }
}

pub fn output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    limit: u64,
) -> Result<BoundedOutput> {
    use std::io::{Read, Seek};
    let mut stdout_file = tempfile::tempfile().context("Create stdout spool")?;
    let mut stderr_file = tempfile::tempfile().context("Create stderr spool")?;
    command
        .stdin(std::process::Stdio::null())
        .stdout(stdout_file.try_clone().context("Clone stdout spool")?)
        .stderr(stderr_file.try_clone().context("Clone stderr spool")?);
    let status = run_with_timeout(command, timeout, TERMINATION_GRACE)?;
    let read_spool = |file: &mut std::fs::File, name: &str| -> Result<Vec<u8>> {
        file.rewind()
            .with_context(|| format!("Rewind {name} spool"))?;
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("Read {name} spool"))?;
        anyhow::ensure!(
            (bytes.len() as u64) <= limit,
            "Command {name} exceeded size limit of {limit} bytes"
        );
        Ok(bytes)
    };
    Ok(BoundedOutput {
        status,
        stdout: read_spool(&mut stdout_file, "stdout")?,
        stderr: read_spool(&mut stderr_file, "stderr")?,
    })
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped
            && let Err(error) = self.terminate(TERMINATION_GRACE)
        {
            log::warn!("Owned launcher cleanup failed: {error:#}");
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::{fs, time::Instant};
    use sysinfo::{Pid, ProcessStatus, ProcessesToUpdate, System};

    fn shell(script: &str, directory: &std::path::Path) -> OwnedChild {
        OwnedChild::spawn(
            Command::new("sh")
                .args(["-c", script])
                .current_dir(directory),
        )
        .unwrap()
    }

    fn ready(path: &std::path::Path) {
        let start = Instant::now();
        while !fs::read(path).is_ok_and(|b| !b.is_empty())
            && start.elapsed() < Duration::from_secs(5)
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            fs::read(path).is_ok_and(|b| !b.is_empty()),
            "Launcher fixture did not become ready"
        );
    }

    fn assert_not_running(pid: u32) {
        let start = Instant::now();
        loop {
            let mut system = System::new().unwrap();
            let pid = Pid::from_u32(pid);
            system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            if system
                .process(pid)
                .is_none_or(|process| process.status() == ProcessStatus::Zombie)
            {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "Descendant survived group cleanup"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn groups_are_dedicated_and_graceful_shutdown_leaves_unrelated_launchers_running() {
        let dir = tempfile::tempdir().unwrap();
        let mut unrelated = OwnedChild::spawn(Command::new("sleep").arg("30")).unwrap();
        let mut launcher = shell(
            "trap 'echo term > terminated; exit 0' TERM; echo ready > ready; while :; do sleep 0.02; done",
            dir.path(),
        );
        ready(&dir.path().join("ready"));
        assert_eq!(
            rustix::process::getpgid(Some(launcher.pid())).unwrap(),
            launcher.pid()
        );
        assert_ne!(launcher.pid(), unrelated.pid());
        assert_ne!(launcher.pid(), rustix::process::getpgrp());
        assert!(
            launcher
                .terminate(Duration::from_millis(150))
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("terminated")).unwrap(),
            "term\n"
        );
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.terminate(Duration::ZERO).unwrap();
    }

    #[test]
    fn kill_escalation_stops_children_and_grandchildren_after_the_grace_period() {
        let dir = tempfile::tempdir().unwrap();
        let mut launcher = shell(
            "trap '' TERM; sh -c 'sleep 30 & echo $! > grandchild; wait' & echo $! > child; wait",
            dir.path(),
        );
        ready(&dir.path().join("grandchild"));
        let child: u32 = fs::read_to_string(dir.path().join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let grandchild: u32 = fs::read_to_string(dir.path().join("grandchild"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let grace = Duration::from_millis(150);
        let start = Instant::now();
        let status = launcher.terminate(grace).unwrap();
        assert!(start.elapsed() >= grace);
        assert_eq!(status.signal(), Some(9));
        assert_not_running(child);
        assert_not_running(grandchild);
        assert!(launcher.reaped);
    }

    #[test]
    fn exited_leader_still_pins_group_ownership_until_descendant_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let mut launcher = shell("sleep 30 & echo $! > child; exit 7", dir.path());
        ready(&dir.path().join("child"));
        let child: u32 = fs::read_to_string(dir.path().join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(status) = launcher.try_wait().unwrap() {
                break status;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(7));
        // macOS does not expose getpgid for a zombie. Re-observing the status with NOWAIT
        // demonstrates that the exited leader remains ours and has not been reaped.
        assert_eq!(launcher.try_wait().unwrap().unwrap().code(), Some(7));
        assert_eq!(
            launcher
                .terminate(Duration::from_millis(100))
                .unwrap()
                .code(),
            Some(7)
        );
        assert_not_running(child);
        assert_eq!(launcher.try_wait().unwrap().unwrap().code(), Some(7));
    }

    #[test]
    fn dropping_an_owned_launcher_cleans_up_its_group() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = shell("sleep 30 & echo $! > child; wait", dir.path());
        ready(&dir.path().join("child"));
        let child: u32 = fs::read_to_string(dir.path().join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(launcher);
        assert_not_running(child);
    }

    #[test]
    fn bounded_output_does_not_block_on_descendant_holding_stdout_and_enforces_limits() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let output = output_with_timeout(
            Command::new("sh")
                .args(["-c", "printf 'wine-11.21\\n'; sleep 30 & echo $! > child"])
                .current_dir(dir.path()),
            Duration::from_secs(2),
            64,
        )
        .unwrap();
        assert!(start.elapsed() < Duration::from_millis(800));
        assert!(output.status.success());
        assert_eq!(output.stdout, b"wine-11.21\n");
        let child: u32 = fs::read_to_string(dir.path().join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_not_running(child);

        let oversized = output_with_timeout(
            Command::new("sh").args(["-c", "printf '12345'"]),
            Duration::from_secs(1),
            4,
        )
        .unwrap_err();
        assert!(oversized.to_string().contains("exceeded size limit"));

        let timed_out = output_with_timeout(
            Command::new("sleep").arg("30"),
            Duration::from_millis(100),
            64,
        )
        .unwrap_err();
        assert!(timed_out.to_string().contains("timed out"));
    }
}
