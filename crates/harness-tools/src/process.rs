//! Bounded child-process capture shared by tools and verification loops.

use std::io;
use std::process::Stdio;
use std::time::Duration;

use harness_core::bounded::BoundedText;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// A completed command whose output was drained without retaining it all.
pub struct CapturedOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Run a command while retaining at most `max_chars` from each output stream.
pub async fn run_bounded(
    command: &mut Command,
    timeout: Duration,
    max_chars: usize,
) -> io::Result<CapturedOutput> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .map(|s| tokio::spawn(drain(s, max_chars)));
    let stderr = child
        .stderr
        .take()
        .map(|s| tokio::spawn(drain(s, max_chars)));

    let (status, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => (status?, false),
        Err(_) => {
            let _ = child.start_kill();
            let status = child.wait().await?;
            (status, true)
        }
    };

    Ok(CapturedOutput {
        code: status.code(),
        stdout: join_reader(stdout).await,
        stderr: join_reader(stderr).await,
        timed_out,
    })
}

async fn drain(mut reader: impl AsyncRead + Unpin, max_chars: usize) -> String {
    let mut kept = BoundedText::new(max_chars);
    let mut bytes = [0u8; 8192];
    loop {
        match reader.read(&mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(n) => kept.push(&String::from_utf8_lossy(&bytes[..n])),
        }
    }
    kept.into_string()
}

async fn join_reader(handle: Option<tokio::task::JoinHandle<String>>) -> String {
    match handle {
        Some(mut handle) => tokio::select! {
            result = &mut handle => result.unwrap_or_default(),
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                // A descendant may have inherited the pipe after the direct
                // child exited. Do not leave a detached drain task around.
                handle.abort();
                String::new()
            }
        },
        None => String::new(),
    }
}
