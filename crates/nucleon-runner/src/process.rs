//! Ownership of a launcher and its Unix process group.
use anyhow::{Context, Result};
use std::{
    process::{Child, Command, ExitStatus},
    thread,
    time::Duration,
};

pub(super) const TERMINATION_GRACE: Duration = Duration::from_millis(300);

pub(super) struct OwnedChild {
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

    #[cfg(test)]
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
    let mut system = System::new();
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
        while !path.is_file() && start.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(path.is_file(), "Launcher fixture did not become ready");
    }

    fn assert_not_running(pid: u32) {
        let start = Instant::now();
        loop {
            let mut system = System::new();
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
}
