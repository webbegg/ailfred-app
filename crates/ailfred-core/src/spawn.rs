//! Starts a child process that answers for itself in macOS's privacy checks (TCC): a plain
//! spawn makes Ailfred the child's "responsible process", so the child, and every command it
//! runs (pi's shell), would use Ailfred's Screen Recording permission. With the
//! responsibility disclaimed it holds no grant of Ailfred's. Its stdin, stdout and stderr are
//! pipes; no other descriptor of Ailfred reaches it.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

unsafe extern "C" {
    /// libSystem (macOS 10.14+): the spawned process becomes responsible for itself. Chromium
    /// and LLDB use it for the same reason.
    fn responsibility_spawnattrs_setdisclaim(attrs: *mut libc::posix_spawnattr_t, disclaim: libc::c_int) -> libc::c_int;
}

/// A child started by [`spawn`], reaped by its owner.
#[derive(Debug)]
pub struct Process {
    pid: libc::pid_t,
    status: Option<ExitStatus>,
}

/// The child and its piped stdio (ours ends: write to `stdin`, read the others).
pub struct Spawned {
    pub process: Process,
    pub stdin: File,
    pub stdout: File,
    pub stderr: File,
}

impl Process {
    #[cfg(test)]
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_with(libc::WNOHANG)
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wait_with(0)?.ok_or_else(|| io::Error::other("waitpid gave no status"))
    }

    /// SIGKILL, unless it's already reaped (so its id can't have been reused).
    pub fn kill(&mut self) -> io::Result<()> {
        self.signal(libc::SIGKILL)
    }

    /// SIGTERM, on the same terms.
    pub fn terminate(&mut self) -> io::Result<()> {
        self.signal(libc::SIGTERM)
    }

    fn signal(&mut self, signal: libc::c_int) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        // SAFETY: an unreaped child of ours.
        if unsafe { libc::kill(self.pid, signal) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wait_with(&mut self, flags: libc::c_int) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        let mut raw = 0;
        loop {
            // SAFETY: `raw` is a valid out-pointer and `pid` our unreaped child.
            match unsafe { libc::waitpid(self.pid, &mut raw, flags) } {
                0 => return Ok(None),
                -1 if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => continue,
                -1 => return Err(io::Error::last_os_error()),
                _ => {
                    self.status = Some(ExitStatus::from_raw(raw));
                    return Ok(self.status);
                }
            }
        }
    }
}

fn check(code: libc::c_int) -> io::Result<()> {
    if code == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(code)) }
}

fn c_string(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL byte in the command"))
}

/// `program` itself when it has a `/`, else the first executable of that name on `path`.
fn resolve(program: &OsStr, path: Option<&OsString>) -> io::Result<PathBuf> {
    if program.as_bytes().contains(&b'/') {
        return Ok(program.into());
    }
    path.into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(program))
        // SAFETY: a NUL-terminated path.
        .find(|candidate| c_string(candidate.as_os_str().as_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{} not found on PATH", program.to_string_lossy())))
}

/// A pipe, both ends closed on exec (the child gets its end through `dup2`): (read, write).
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for both.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: just made, owned by nothing else.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&read, &write] {
        // SAFETY: an open descriptor.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read, write))
}

struct Attributes(libc::posix_spawnattr_t);

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: made by `posix_spawnattr_init`.
        unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
    }
}

struct FileActions(libc::posix_spawn_file_actions_t);

impl Drop for FileActions {
    fn drop(&mut self) {
        // SAFETY: made by `posix_spawn_file_actions_init`.
        unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
    }
}

/// Starts `command` (its program, arguments, environment changes and folder) disclaimed.
pub fn spawn(command: &Command) -> io::Result<Spawned> {
    let mut env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => env.insert(key.to_owned(), value.to_owned()),
            None => env.remove(key),
        };
    }
    let program = resolve(command.get_program(), env.get(OsStr::new("PATH")))?;
    let program = c_string(program.as_os_str().as_bytes())?;
    let argv = std::iter::once(command.get_program()).chain(command.get_args()).map(|a| c_string(a.as_bytes())).collect::<io::Result<Vec<_>>>()?;
    let envp = env
        .into_iter()
        .map(|(key, value)| {
            let mut entry = key.into_vec();
            entry.push(b'=');
            entry.extend(value.into_vec());
            c_string(&entry)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let mut argv_ptrs: Vec<*mut libc::c_char> = argv.iter().map(|a| a.as_ptr().cast_mut()).chain([std::ptr::null_mut()]).collect();
    let mut envp_ptrs: Vec<*mut libc::c_char> = envp.iter().map(|e| e.as_ptr().cast_mut()).chain([std::ptr::null_mut()]).collect();
    let cwd = command.get_current_dir().map(|d| c_string(d.as_os_str().as_bytes())).transpose()?;

    let (stdin_read, stdin_write) = pipe()?;
    let (stdout_read, stdout_write) = pipe()?;
    let (stderr_read, stderr_write) = pipe()?;

    // SAFETY: each struct is initialized before use and destroyed by its guard; every pointer
    // passed lives until `posix_spawn` returns.
    let pid = unsafe {
        let mut actions = FileActions(std::mem::zeroed());
        check(libc::posix_spawn_file_actions_init(&mut actions.0))?;
        check(libc::posix_spawn_file_actions_adddup2(&mut actions.0, stdin_read.as_raw_fd(), 0))?;
        check(libc::posix_spawn_file_actions_adddup2(&mut actions.0, stdout_write.as_raw_fd(), 1))?;
        check(libc::posix_spawn_file_actions_adddup2(&mut actions.0, stderr_write.as_raw_fd(), 2))?;
        if let Some(cwd) = &cwd {
            check(libc::posix_spawn_file_actions_addchdir_np(&mut actions.0, cwd.as_ptr()))?;
        }
        let mut attributes = Attributes(std::mem::zeroed());
        check(libc::posix_spawnattr_init(&mut attributes.0))?;
        // Only stdin, stdout and stderr reach the child, with every signal at its default and
        // none blocked (Rust ignores SIGPIPE in its own process).
        let flags = libc::POSIX_SPAWN_CLOEXEC_DEFAULT | libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_SETSIGMASK;
        check(libc::posix_spawnattr_setflags(&mut attributes.0, flags as libc::c_short))?;
        let mut all: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut all);
        check(libc::posix_spawnattr_setsigdefault(&mut attributes.0, &all))?;
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        check(libc::posix_spawnattr_setsigmask(&mut attributes.0, &none))?;
        check(responsibility_spawnattrs_setdisclaim(&mut attributes.0, 1))?;
        let mut pid = 0;
        check(libc::posix_spawn(&mut pid, program.as_ptr(), &actions.0, &attributes.0, argv_ptrs.as_mut_ptr(), envp_ptrs.as_mut_ptr()))?;
        pid
    };
    drop((stdin_read, stdout_write, stderr_write));
    Ok(Spawned { process: Process { pid, status: None }, stdin: stdin_write.into(), stdout: stdout_read.into(), stderr: stderr_read.into() })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    fn output(mut spawned: Spawned) -> (String, ExitStatus) {
        drop(spawned.stdin);
        let mut out = String::new();
        spawned.stdout.read_to_string(&mut out).unwrap();
        (out, spawned.process.wait().unwrap())
    }

    #[test]
    fn it_runs_with_the_commands_folder_environment_and_arguments() {
        let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let mut command = sh("pwd; echo \"$AILFRED_TEST:${HOME:-none}\"; echo \"$1\"");
        command.arg("x").arg("one two").current_dir(&dir).env("AILFRED_TEST", "set").env_remove("HOME");
        let (out, status) = output(spawn(&command).expect("spawned"));
        assert!(status.success());
        assert_eq!(out, format!("{}\nset:none\none two\n", dir.display()));
    }

    #[test]
    fn its_stdio_is_piped() {
        let mut spawned = spawn(&sh("read line; echo \"got $line\"; echo oops >&2; exit 3")).expect("spawned");
        spawned.stdin.write_all(b"hi\n").unwrap();
        let mut err = String::new();
        spawned.stderr.read_to_string(&mut err).unwrap();
        let (out, status) = output(spawned);
        assert_eq!((out.as_str(), err.as_str(), status.code()), ("got hi\n", "oops\n", Some(3)));
    }

    #[test]
    fn a_program_is_found_on_the_childs_path() {
        let mut command = Command::new("true");
        command.env("PATH", "/usr/bin:/bin");
        assert!(output(spawn(&command).expect("found")).1.success());
        let mut missing = Command::new("no-such-program-ailfred");
        missing.env("PATH", "/nowhere");
        assert!(spawn(&missing).is_err());
    }

    #[test]
    fn the_child_gets_default_signal_handling() {
        // Rust ignores SIGPIPE in its own process; the child mustn't: `yes` ends by the signal.
        let mut spawned = spawn(&sh("(yes; echo \"yes-exit=$?\" >&2) | head -n1 >/dev/null")).unwrap();
        let mut err = String::new();
        spawned.stderr.read_to_string(&mut err).unwrap();
        output(spawned);
        assert_eq!(err, "yes-exit=141\n");
    }

    #[test]
    fn no_other_descriptor_of_ours_reaches_it() {
        // One of ours, open and not close-on-exec.
        let (keep, _w) = pipe().unwrap();
        let fd = keep.as_raw_fd();
        unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
        let (out, _) = output(spawn(&sh(&format!("if [ -e /dev/fd/{fd} ]; then echo open; else echo closed; fi"))).unwrap());
        assert_eq!(out, "closed\n");
    }

    #[test]
    fn killing_and_reaping() {
        let mut spawned = spawn(&sh("while :; do sleep 0.05; done")).expect("spawned");
        assert_eq!(spawned.process.try_wait().unwrap(), None);
        spawned.process.terminate().unwrap();
        let status = spawned.process.wait().unwrap();
        assert!(!status.success());
        assert!(spawned.process.kill().is_ok(), "reaped: nothing to signal");
        assert_eq!(spawned.process.try_wait().unwrap(), Some(status));
    }
}
