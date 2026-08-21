//! A child process that dies with the supervisor.
//!
//! `arc dev` owns two children: the Node process running Vite, and the
//! application binary. Both hold an IPC endpoint. An orphan holding a named
//! pipe makes the *next* `arc dev` fail to bind, and an orphaned Vite keeps
//! a file watcher running over the project for as long as the terminal
//! lives -- so neither is allowed to outlive the supervisor, whether it
//! exits normally, on Ctrl-C, or by an error part-way through startup.
//!
//! # The pipe that is never written to
//!
//! [`Drop`] covers every exit the supervisor gets to run code for. It does
//! not cover the ones it does not: `kill -9`, End Task, a panic in a thread
//! that aborts the process, the IDE that closes the terminal. Those leave
//! exactly the orphans described above, and on Windows a named pipe held by
//! an invisible process is a genuinely confusing thing to debug.
//!
//! So each child is given a stdin pipe that the supervisor holds open and
//! never writes to. The supervisor is the only writer, so the moment it
//! stops existing -- by any means, including the ones it cannot intercept --
//! the operating system closes the write end and the child's next read of
//! stdin returns end-of-file. Both children watch for that and exit. It
//! costs one handle per child and no polling, and it is the only mechanism
//! that survives a kill the supervisor never sees.
//!
//! The child half is [`crate::application::serve_ipc::exit_when_orphaned`]
//! for the application and the `process.stdin` watch in the generated
//! `vite-ipc.mjs` for Vite.
//!
//!
//! # Why `std::process` and not `tokio::process`
//!
//! Tokio's `process` feature is not enabled in this crate's manifest, and
//! `arc dev` is not the place to add it: spawning and killing are two
//! syscalls, and the one genuinely blocking operation -- waiting for
//! `cargo build` -- runs on `spawn_blocking` where blocking is what the
//! thread is for.

use std::process::{Child, Command, Stdio};

/// A spawned child process, killed when this value is dropped.
///
/// Dropping is the only teardown path that cannot be forgotten: every early
/// return in the startup sequence, the `?` on a build failure, and the
/// normal exit all run it.
pub(crate) struct ChildGuard {
    /// What this process is, for the one line we print if it will not die.
    label: &'static str,
    child: Option<Child>,
}

impl ChildGuard {
    /// Spawn `command`, taking ownership of the resulting process.
    ///
    /// # Errors
    ///
    /// `io::Error` if the program cannot be spawned -- almost always "not on
    /// PATH", which is the message the caller turns into advice.
    pub(crate) fn spawn(label: &'static str, command: &mut Command) -> std::io::Result<Self> {
        let child = command.spawn()?;
        Ok(Self {
            label,
            child: Some(child),
        })
    }

    /// The process id, for diagnostics.
    pub(crate) fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Has the process exited on its own?
    ///
    /// Used to notice a backend that crashed at boot instead of waiting the
    /// full readiness timeout for an endpoint that will never appear.
    pub(crate) fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.as_mut()?.try_wait().ok().flatten()
    }

    /// Kill the process and reap it.
    ///
    /// Reaping matters as much as killing: the endpoint the child holds is
    /// released by the operating system when the process is gone, not when
    /// the kill is requested, and the next spawn binds that same endpoint.
    pub(crate) fn stop(&mut self) {
        // Read the id before taking the child: a process that will not die
        // has to be killed by hand, and the number is the only thing that
        // makes that possible.
        let pid = self.id();
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.kill();
        if let Err(error) = child.wait() {
            let pid = pid.map_or_else(|| String::from("unknown"), |pid| pid.to_string());
            eprintln!(
                "arc dev: could not reap the {} process (pid {pid}): {error}. \
                 It is still holding an IPC endpoint; kill it before starting `arc dev` again.",
                self.label
            );
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A `Command` whose output is the terminal's, ready for [`ChildGuard::spawn`].
///
/// The application's logs and Vite's startup line belong in the developer's
/// terminal unedited; capturing them to re-print would only add latency and
/// lose colour.
pub(crate) fn inherited(program: &str) -> Command {
    let mut command = Command::new(program);
    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    // Piped, and never written to: this is the death pledge described in the
    // module header. It is deliberately not `Stdio::inherit()` -- sharing the
    // supervisor's own stdin would let a child steal the developer's Ctrl-C
    // keystrokes, and it would stay open after the supervisor died.
    command.stdin(Stdio::piped());
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guard_reports_the_pid_of_a_live_child() {
        let mut command = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "exit 0"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "exit 0"]);
            c
        };
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let mut guard = ChildGuard::spawn("test", &mut command).expect("spawn should succeed");
        assert!(guard.id().is_some());
        guard.stop();
        assert!(guard.id().is_none());
    }

    #[test]
    fn a_child_is_given_a_stdin_the_supervisor_holds_open() {
        // The death pledge: the handle has to be a pipe the supervisor owns,
        // because closing it is the signal. `Stdio::null()` would be closed
        // from the start and `Stdio::inherit()` would outlive the supervisor,
        // and either way the child never learns that it was orphaned.
        let mut command = inherited(if cfg!(windows) { "cmd" } else { "sh" });
        command.args(if cfg!(windows) {
            ["/C", "exit 0"]
        } else {
            ["-c", "exit 0"]
        });
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let mut guard = ChildGuard::spawn("test", &mut command).expect("spawn should succeed");
        assert!(
            guard.child.as_ref().is_some_and(|c| c.stdin.is_some()),
            "the write end must stay with the guard; dropping it early would \
             tell the child it was orphaned while the supervisor is alive"
        );
        guard.stop();
    }

    #[test]
    fn stopping_twice_is_not_an_error() {
        let mut command = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "exit 0"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "exit 0"]);
            c
        };
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let mut guard = ChildGuard::spawn("test", &mut command).expect("spawn should succeed");
        guard.stop();
        guard.stop();
    }

    #[test]
    fn spawning_a_program_that_is_not_installed_is_an_error() {
        let mut command = Command::new("arcature-no-such-program-exists");
        assert!(ChildGuard::spawn("test", &mut command).is_err());
    }
}
