//! A child process (curl or claude) that a run owns and Stop can end.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How much of a process's stderr is kept.
const STDERR_CAP: u64 = 16 * 1024;

#[derive(Default)]
struct Shared {
    child: Mutex<Option<Child>>,
    cancelled: AtomicBool,
    pid: AtomicU32,
}

/// A running model call: `cancel` ends its process.
#[derive(Clone, Default)]
pub struct Run {
    shared: Arc<Shared>,
}

pub struct Outcome {
    pub success: bool,
    pub stderr: String,
}

impl Run {
    pub fn new() -> Run {
        Run::default()
    }

    fn child(&self) -> MutexGuard<'_, Option<Child>> {
        self.shared.child.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Kills the process (it leads a group of its own, so what it started too) and reaps it.
    /// What it still says is dropped; a run cancelled before its process starts never starts it.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
        if let Some(child) = self.child().as_mut() {
            // The whole group, even once the leader is gone: what it started may still hold its
            // output open. While a member lives, no new process can take the group's id.
            // SAFETY: killpg only sends a signal.
            unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Acquire)
    }

    /// The process was started and has ended (`try_wait` says so).
    #[cfg(test)]
    pub fn exited(&self) -> bool {
        self.child().as_mut().is_some_and(|c| c.try_wait().is_ok_and(|status| status.is_some()))
    }

    /// The process's id, once started.
    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        Some(self.shared.pid.load(Ordering::Acquire)).filter(|pid| *pid != 0)
    }
}

/// Runs `cmd` with `input` on its stdin, calling `on_line` for each line of its stdout, until
/// it ends or `run` is cancelled.
pub fn run(run: &Run, mut cmd: Command, input: Vec<u8>, mut on_line: impl FnMut(&str)) -> io::Result<Outcome> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let (stdin, stdout, stderr) = {
        // Under the lock, so a cancel lands before the spawn (and none starts) or after the
        // child is stored (and kills it).
        let mut slot = run.child();
        if run.cancelled() {
            return Ok(Outcome { success: false, stderr: String::new() });
        }
        let mut child = cmd.spawn()?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        run.shared.pid.store(child.id(), Ordering::Release);
        *slot = Some(child);
        pipes
    };
    // Written on its own thread: a child that answers before it has read everything mustn't
    // block on a full pipe while we do.
    let writer = stdin.map(|mut stdin| std::thread::spawn(move || stdin.write_all(&input)));
    let errors = stderr.map(|stderr| {
        std::thread::spawn(move || {
            let mut said = Vec::new();
            let mut stderr = stderr;
            let _ = (&mut stderr).take(STDERR_CAP).read_to_end(&mut said);
            // Past the cap, keep reading: a closed pipe would kill the child at its next write.
            let _ = io::copy(&mut stderr, &mut io::sink());
            String::from_utf8_lossy(&said).into_owned()
        })
    });
    if let Some(stdout) = stdout {
        for line in BufReader::new(stdout).split(b'\n') {
            let Ok(line) = line else { break };
            on_line(&String::from_utf8_lossy(&line));
        }
    }
    let success = loop {
        // Without the lock between looks, so a cancel can take it.
        let status = run.child().as_mut().and_then(|c| c.try_wait().ok().flatten());
        if let Some(status) = status {
            break status.success();
        }
        if run.cancelled() {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stderr = errors.and_then(|t| t.join().ok()).unwrap_or_default();
    Ok(Outcome { success, stderr })
}
