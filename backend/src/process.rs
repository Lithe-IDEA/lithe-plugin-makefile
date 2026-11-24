//! Runs `make` with process-group isolation and cancellation support so
//! cancelling a build request stops `make` and any child processes it spawned.

use lithe_plugin_sdk::{Cancellation, FunctionError, PluginError};
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(50);
const DRAIN: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct Invocation {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub directory: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
}

enum Message {
    Chunk(Vec<u8>),
    Closed,
}

/// Runs `invocation` to completion, streaming stdout and stderr bytes to `on_chunk`.
pub fn run(
    invocation: &Invocation,
    cancellation: &Cancellation,
    mut on_chunk: impl FnMut(&[u8]) -> Result<(), FunctionError>,
) -> Result<i32, FunctionError> {
    cancellation.check()?;
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.arguments)
        .current_dir(&invocation.directory)
        .envs(invocation.environment.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate(&mut command);

    let mut child = command.spawn().map_err(|error| {
        PluginError::new(
            "makefile.startFailed",
            format!("make could not start: {}.", error.kind()),
        )
    })?;

    let (sender, receiver) = mpsc::channel();
    let mut open = 0;
    if let Some(stdout) = child.stdout.take() {
        pump(stdout, sender.clone());
        open += 1;
    }
    if let Some(stderr) = child.stderr.take() {
        pump(stderr, sender.clone());
        open += 1;
    }
    drop(sender);

    let mut exited: Option<(ExitStatus, Instant)> = None;
    loop {
        if cancellation.is_cancelled() {
            stop(&mut child, exited.is_some());
            return Err(FunctionError::Cancelled);
        }
        if open == 0 {
            break;
        }
        if let Some((_, at)) = exited {
            if at.elapsed() > DRAIN {
                break;
            }
        }
        match receiver.recv_timeout(POLL) {
            Ok(Message::Chunk(bytes)) => {
                if let Err(err) = on_chunk(&bytes) {
                    stop(&mut child, exited.is_some());
                    return Err(err);
                }
            }
            Ok(Message::Closed) => open -= 1,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => open = 0,
        }
        if exited.is_none() {
            if let Ok(Some(status)) = child.try_wait() {
                exited = Some((status, Instant::now()));
            }
        }
    }

    let status = match exited {
        Some((status, _)) => status,
        None => loop {
            if cancellation.is_cancelled() {
                stop(&mut child, false);
                return Err(FunctionError::Cancelled);
            }
            if let Ok(Some(status)) = child.try_wait() {
                break status;
            }
            thread::sleep(POLL);
        },
    };
    Ok(status.code().unwrap_or(1))
}

fn pump(mut reader: impl Read + Send + 'static, sender: mpsc::Sender<Message>) {
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if sender.send(Message::Chunk(buffer[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = sender.send(Message::Closed);
    });
}

#[cfg(unix)]
fn isolate(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn isolate(_command: &mut Command) {}

#[cfg(unix)]
fn stop(child: &mut Child, already_exited: bool) {
    let pid = child.id() as libc::pid_t;
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    if !already_exited {
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.wait();
}

#[cfg(windows)]
fn stop(child: &mut Child, already_exited: bool) {
    if !already_exited {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(any(unix, windows)))]
fn stop(child: &mut Child, _already_exited: bool) {
    let _ = child.kill();
    let _ = child.wait();
}
