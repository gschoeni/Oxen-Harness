//! Background shell tasks: long-running commands that outlive a single tool
//! call.
//!
//! A task is spawned in its own process group with its combined output
//! streamed to a log file on disk (complete) and into bounded in-memory
//! tails (what tool results show). The model interacts with tasks through
//! three tools sharing one [`BackgroundTasks`] registry:
//!
//! - `run_shell` with `is_background: true` starts a task and returns its id
//!   immediately; a *foreground* command that hits its timeout is converted
//!   to a background task instead of being killed, so a slow build or a dev
//!   server that "hangs" the shell is never lost work.
//! - [`TaskOutputTool`] reports a task's status plus the output produced
//!   since the last check (a moving cursor into the log file).
//! - [`KillTaskTool`] terminates a task's whole process group.
//!
//! Tasks live as long as the host process: children are killed when the
//! registry's handles drop, so a crashed or exited host doesn't strand
//! shells (grandchildren that re-parented themselves may survive — a dev
//! server managed by `harness-preview` has its own lifecycle for that).

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use harness_core::bounded::BoundedText;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{watch, Mutex};

use crate::{ToolError, TypedTool};

/// Tool name for [`TaskOutputTool`].
pub const TASK_OUTPUT_TOOL: &str = "task_output";
/// Tool name for [`KillTaskTool`].
pub const KILL_TASK_TOOL: &str = "kill_task";

/// The most of a task's log a single `task_output` call returns.
const TASK_OUTPUT_TAIL_CHARS: usize = 30_000;

/// The most simultaneously *running* tasks a timeout may auto-background
/// into. Past this, a timed-out foreground command is killed (the classic
/// behavior) instead of converted — auto-backgrounding exists to save slow
/// builds and accidentally-foregrounded servers, not to let runaway commands
/// accumulate without bound.
pub const MAX_AUTO_BACKGROUND_TASKS: usize = 8;

/// How a finished task ended.
#[derive(Debug, Clone, Copy)]
pub struct TaskExit {
    /// The exit code; `None` means it died on a signal (e.g. `kill_task`).
    pub code: Option<i32>,
}

/// One live (or finished-but-unqueried) background task.
struct TaskEntry {
    command: String,
    log_path: PathBuf,
    /// Process-group leader (the task is spawned as its own group leader),
    /// so a kill reaches the whole tree. `None` where unsupported.
    pid: Option<i32>,
    /// Byte offset into the log file up to which `task_output` has reported.
    cursor: u64,
    /// Resolves once, to the exit status, when the process ends.
    done: watch::Receiver<Option<TaskExit>>,
    /// Set the instant the child is reaped — *before* `done` resolves (the
    /// pumps drain first). `kill` checks this so it never signals a process
    /// group whose leader pid the OS may already have recycled.
    reaped: Arc<std::sync::atomic::AtomicBool>,
    /// Bounded tails per stream, for foreground-completion formatting.
    stdout_tail: Arc<StdMutex<Option<BoundedText>>>,
    stderr_tail: Arc<StdMutex<Option<BoundedText>>>,
}

/// The shared task registry: one per tool set (i.e. per session's registry),
/// shared by `run_shell`, `task_output`, and `kill_task`.
pub struct BackgroundTasks {
    log_dir: PathBuf,
    next_id: AtomicU64,
    tasks: Mutex<HashMap<u64, TaskEntry>>,
}

impl BackgroundTasks {
    /// A registry writing task logs under `log_dir` (created on first spawn).
    pub fn new(log_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            log_dir,
            next_id: AtomicU64::new(0),
            tasks: Mutex::new(HashMap::new()),
        })
    }

    /// A registry logging under the OS temp directory — the default used by
    /// the shipped tool set (task logs are diagnostics, not project files).
    ///
    /// One directory per registry instance: task ids are per-registry, so a
    /// shared directory would let two sessions (or two harness processes)
    /// both create `task-1.log` and truncate each other's logs. Pid plus a
    /// process-wide counter make the path unique without a uuid dependency.
    pub fn in_temp() -> Arc<Self> {
        static INSTANCE: AtomicU64 = AtomicU64::new(0);
        let instance = INSTANCE.fetch_add(1, Ordering::Relaxed);
        Self::new(
            std::env::temp_dir()
                .join("oxen-harness-tasks")
                .join(format!("{}-{instance}", std::process::id())),
        )
    }

    /// Spawn `command` (via the platform shell, cwd `root`) as a background
    /// task, returning its id. Output streams to the task's log file and into
    /// bounded in-memory tails; the child gets no stdin.
    pub async fn spawn(
        &self,
        command: &str,
        root: &Path,
        max_tail: usize,
    ) -> Result<u64, ToolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        tokio::fs::create_dir_all(&self.log_dir)
            .await
            .map_err(|e| ToolError::Execution(format!("create task log dir: {e}")))?;
        let log_path = self.log_dir.join(format!("task-{id}.log"));
        let log = Arc::new(Mutex::new(
            tokio::fs::File::create(&log_path)
                .await
                .map_err(|e| ToolError::Execution(format!("create task log: {e}")))?,
        ));

        let mut cmd = crate::shell::shell_command(command);
        cmd.current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Its own process group, so `kill` can reach the whole command tree
        // (a `sh -c` wrapping a server, the server's own workers, …).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.as_std_mut().process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Execution(format!("spawn `{command}`: {e}")))?;
        let pid = child.id().map(|p| p as i32);

        let stdout_tail = Arc::new(StdMutex::new(Some(BoundedText::new(max_tail))));
        let stderr_tail = Arc::new(StdMutex::new(Some(BoundedText::new(max_tail))));
        let out_pump = child
            .stdout
            .take()
            .map(|s| tokio::spawn(pump(s, log.clone(), stdout_tail.clone())));
        let err_pump = child
            .stderr
            .take()
            .map(|s| tokio::spawn(pump(s, log.clone(), stderr_tail.clone())));

        let (done_tx, done_rx) = watch::channel(None);
        let reaped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reaped_flag = reaped.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            // The child is reaped: once the last group member is gone the OS
            // may recycle the pid, so flag it now — before the (possibly
            // slow) pump drain — to close the window where `kill` could
            // signal a recycled process group.
            reaped_flag.store(true, Ordering::Release);
            // Let the pumps drain what the pipes still hold before reporting
            // done, so "exited" status never races its own final output.
            if let Some(pump) = out_pump {
                let _ = pump.await;
            }
            if let Some(pump) = err_pump {
                let _ = pump.await;
            }
            let _ = done_tx.send(Some(TaskExit {
                code: status.ok().and_then(|s| s.code()),
            }));
        });

        self.tasks.lock().await.insert(
            id,
            TaskEntry {
                command: command.to_string(),
                log_path,
                pid,
                cursor: 0,
                done: done_rx,
                reaped,
                stdout_tail,
                stderr_tail,
            },
        );
        Ok(id)
    }

    /// How many tasks are still running.
    pub async fn running_count(&self) -> usize {
        self.tasks
            .lock()
            .await
            .values()
            .filter(|e| e.done.borrow().is_none())
            .count()
    }

    /// The last `max_bytes` of a task's log so far, without moving the
    /// `task_output` cursor — for the auto-background notice, so the model
    /// sees what the command was doing when it was converted.
    pub async fn peek_tail(&self, id: u64, max_bytes: u64) -> String {
        let log_path = {
            let tasks = self.tasks.lock().await;
            match tasks.get(&id) {
                Some(entry) => entry.log_path.clone(),
                None => return String::new(),
            }
        };
        let len = tokio::fs::metadata(&log_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let (text, _, _) = read_since(&log_path, len.saturating_sub(max_bytes), false).await;
        text
    }

    /// Wait up to `timeout` for task `id` to exit. `None` means it's still
    /// running when time ran out (the caller decides what that means —
    /// `run_shell` reports it as auto-backgrounded).
    pub async fn wait(&self, id: u64, timeout: Duration) -> Option<TaskExit> {
        let mut done = self.tasks.lock().await.get(&id)?.done.clone();
        tokio::time::timeout(timeout, async move {
            loop {
                if let Some(exit) = *done.borrow() {
                    return exit;
                }
                if done.changed().await.is_err() {
                    return TaskExit { code: None };
                }
            }
        })
        .await
        .ok()
    }

    /// Remove a finished task and hand back its bounded stdout/stderr tails —
    /// the foreground-completion path, matching `run_shell`'s classic output.
    /// The log file goes with the entry; the streams are its full story.
    pub async fn take_streams(&self, id: u64) -> Option<(TaskExit, String, String)> {
        let entry = self.tasks.lock().await.remove(&id)?;
        let exit = (*entry.done.borrow())?;
        let take = |tail: &Arc<StdMutex<Option<BoundedText>>>| {
            tail.lock()
                .ok()
                .and_then(|mut t| t.take())
                .map(BoundedText::into_string)
                .unwrap_or_default()
        };
        let streams = (exit, take(&entry.stdout_tail), take(&entry.stderr_tail));
        let _ = tokio::fs::remove_file(&entry.log_path).await;
        Some(streams)
    }

    /// Status plus the output appended since the last check (or the last
    /// `TASK_OUTPUT_TAIL_CHARS` of it, when more arrived than fits). Once
    /// an exited task's output has been fully delivered, the entry and its
    /// log file are retired — a long session doesn't accumulate them.
    pub async fn output(&self, id: u64) -> Result<String, ToolError> {
        // Snapshot under the lock, then do the file I/O unlocked, so one slow
        // log read can't stall spawn/kill/wait for every other task.
        let (command, log_path, cursor, done) = {
            let tasks = self.tasks.lock().await;
            let entry = tasks
                .get(&id)
                .ok_or_else(|| ToolError::Execution(format!("no background task {id}")))?;
            (
                entry.command.clone(),
                entry.log_path.clone(),
                entry.cursor,
                entry.done.clone(),
            )
        };
        // Status read *before* the log: `done` is set only after the pumps
        // finish flushing, so "exited" here guarantees the log is complete —
        // a full read below means everything was delivered.
        let exit = *done.borrow();
        let status = match exit {
            Some(TaskExit { code: Some(code) }) => format!("exited with code {code}"),
            Some(TaskExit { code: None }) => "exited on a signal".to_string(),
            None => "running".to_string(),
        };
        let (delta, new_cursor, skipped) = read_since(&log_path, cursor, exit.is_some()).await;

        let retire = exit.is_some() && skipped == 0;
        {
            let mut tasks = self.tasks.lock().await;
            if retire {
                tasks.remove(&id);
            } else if let Some(entry) = tasks.get_mut(&id) {
                // Concurrent checks race benignly: the cursor only advances.
                entry.cursor = entry.cursor.max(new_cursor);
            }
        }
        if retire {
            let _ = tokio::fs::remove_file(&log_path).await;
        }

        let skipped_note = if skipped > 0 {
            format!(
                "\n… [{skipped} bytes of new output omitted — full log: {}]",
                log_path.display()
            )
        } else {
            String::new()
        };
        let retired_note = if retire {
            "\n(final output delivered — task entry retired)"
        } else {
            ""
        };
        let body = if delta.is_empty() {
            "(no new output)".to_string()
        } else {
            delta
        };
        Ok(format!(
            "task {id} ({command}): {status}{retired_note}\n--- new output since last check ---{skipped_note}\n{body}"
        ))
    }

    /// Kill task `id`'s whole process group. The entry stays queryable so a
    /// final `task_output` can confirm the exit and read the last output.
    pub async fn kill(&self, id: u64) -> Result<String, ToolError> {
        let tasks = self.tasks.lock().await;
        let entry = tasks
            .get(&id)
            .ok_or_else(|| ToolError::Execution(format!("no background task {id}")))?;
        // `reaped` flips the instant the leader is waited on — after that the
        // OS may recycle the pid, so signalling the group is off the table.
        if entry.done.borrow().is_some() || entry.reaped.load(Ordering::Acquire) {
            return Ok(format!("task {id} had already exited"));
        }
        #[cfg(unix)]
        {
            if let Some(pid) = entry.pid {
                // Negative pid = the whole process group (the task is its
                // group's leader — see `spawn`). SAFETY: plain syscall on a
                // pid we spawned; the worst a stale pid can do is ESRCH.
                #[allow(unsafe_code)]
                unsafe {
                    libc::kill(-pid, libc::SIGKILL)
                };
                return Ok(format!("kill signal sent to task {id} ({})", entry.command));
            }
        }
        Err(ToolError::Execution(format!(
            "task {id} cannot be killed on this platform"
        )))
    }
}

/// Copy one stream into the shared log file (raw bytes) and the bounded
/// in-memory tail (text). The tail decodes with a carry buffer so a
/// multibyte character split across two 8 KiB reads isn't torn into
/// replacement chars.
async fn pump(
    mut reader: impl AsyncRead + Unpin,
    log: Arc<Mutex<tokio::fs::File>>,
    tail: Arc<StdMutex<Option<BoundedText>>>,
) {
    let mut bytes = [0u8; 8192];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        match reader.read(&mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                {
                    let mut file = log.lock().await;
                    let _ = file.write_all(&bytes[..n]).await;
                    let _ = file.flush().await;
                }
                carry.extend_from_slice(&bytes[..n]);
                // Decode everything but a trailing incomplete sequence, which
                // stays in the carry for the next read to complete.
                let take = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    // Genuinely invalid bytes: decode lossily and move on.
                    Err(_) => carry.len(),
                };
                if take > 0 {
                    if let Ok(mut tail) = tail.lock() {
                        if let Some(tail) = tail.as_mut() {
                            tail.push(&String::from_utf8_lossy(&carry[..take]));
                        }
                    }
                    carry.drain(..take);
                }
            }
        }
    }
    // A stream that ended mid-character still shows its last bytes.
    if !carry.is_empty() {
        if let Ok(mut tail) = tail.lock() {
            if let Some(tail) = tail.as_mut() {
                tail.push(&String::from_utf8_lossy(&carry));
            }
        }
    }
}

/// Read the log from `cursor` to its end, capped to the last
/// [`TASK_OUTPUT_TAIL_CHARS`] bytes of the delta. Returns the text, the new
/// cursor, and how many bytes of the delta were skipped.
///
/// The cursor advances to what was **actually read** (not the pre-read file
/// length — a still-running task appends during the read, and using the
/// stale length would re-emit the overlap on the next check). Unless
/// `final_read`, a trailing incomplete UTF-8 sequence is held back for the
/// next call to complete (the pump writes raw 8 KiB chunks, so EOF can land
/// mid-character).
async fn read_since(path: &Path, cursor: u64, final_read: bool) -> (String, u64, u64) {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return (String::new(), cursor, 0);
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(cursor);
    if len <= cursor {
        return (String::new(), cursor, 0);
    }
    let delta = len - cursor;
    let (start, skipped) = if delta > TASK_OUTPUT_TAIL_CHARS as u64 {
        (
            len - TASK_OUTPUT_TAIL_CHARS as u64,
            delta - TASK_OUTPUT_TAIL_CHARS as u64,
        )
    } else {
        (cursor, 0)
    };
    if file.seek(SeekFrom::Start(start)).await.is_err() {
        return (String::new(), cursor, 0);
    }
    let mut bytes = Vec::with_capacity((len - start) as usize);
    if file.read_to_end(&mut bytes).await.is_err() {
        return (String::new(), cursor, 0);
    }
    // A skip lands at an arbitrary byte offset: step past any leading UTF-8
    // continuation bytes so the tail doesn't open on a torn character.
    let lead = if skipped > 0 {
        bytes
            .iter()
            .take(4)
            .position(|b| (b & 0xC0) != 0x80)
            .unwrap_or_else(|| bytes.len().min(4))
    } else {
        0
    };
    // Hold back a trailing incomplete sequence for the next call.
    let mut keep = bytes.len();
    if !final_read {
        if let Err(e) = std::str::from_utf8(&bytes[lead..]) {
            if e.error_len().is_none() {
                keep = lead + e.valid_up_to();
            }
        }
    }
    let text = String::from_utf8_lossy(&bytes[lead..keep]).into_owned();
    (text, start + keep as u64, skipped)
}

/// Arguments to `task_output`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct TaskOutputArgs {
    /// The id `run_shell` returned when the task started (or auto-backgrounded).
    pub task_id: u64,
}

/// Check on a background task: status + output since the last check.
pub struct TaskOutputTool {
    tasks: Arc<BackgroundTasks>,
}

impl TaskOutputTool {
    pub fn new(tasks: Arc<BackgroundTasks>) -> Self {
        Self { tasks }
    }
}

#[async_trait]
impl TypedTool for TaskOutputTool {
    const NAME: &'static str = TASK_OUTPUT_TOOL;
    type Args = TaskOutputArgs;

    fn description(&self) -> &str {
        "Check a background task started by run_shell: its status (running/exited) and the \
         output produced since the last check. Do not poll in a sleep loop — check between \
         other useful work."
    }

    async fn run(&self, args: TaskOutputArgs) -> Result<String, ToolError> {
        self.tasks.output(args.task_id).await
    }
}

/// Arguments to `kill_task`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct KillTaskArgs {
    /// The id of the background task to terminate.
    pub task_id: u64,
}

/// Terminate a background task (its whole process group).
pub struct KillTaskTool {
    tasks: Arc<BackgroundTasks>,
}

impl KillTaskTool {
    pub fn new(tasks: Arc<BackgroundTasks>) -> Self {
        Self { tasks }
    }
}

#[async_trait]
impl TypedTool for KillTaskTool {
    const NAME: &'static str = KILL_TASK_TOOL;
    type Args = KillTaskArgs;

    fn description(&self) -> &str {
        "Terminate a background task started by run_shell (kills its whole process group)."
    }

    async fn run(&self, args: KillTaskArgs) -> Result<String, ToolError> {
        self.tasks.kill(args.task_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_registry(dir: &Path) -> Arc<BackgroundTasks> {
        BackgroundTasks::new(dir.join("tasks"))
    }

    #[tokio::test]
    async fn background_task_streams_output_and_reports_exit() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = temp_registry(dir.path());
        let id = tasks
            .spawn("echo hello-bg", dir.path(), 1000)
            .await
            .unwrap();
        // Wait for it to finish, then read.
        let exit = tasks.wait(id, Duration::from_secs(10)).await.unwrap();
        assert_eq!(exit.code, Some(0));
        let report = tasks.output(id).await.unwrap();
        assert!(report.contains("exited with code 0"), "{report}");
        assert!(report.contains("hello-bg"), "{report}");
        // Everything was delivered, so the entry (and its log) were retired.
        assert!(report.contains("task entry retired"), "{report}");
        assert!(
            tasks.output(id).await.is_err(),
            "retired task should be gone"
        );
    }

    #[tokio::test]
    async fn kill_terminates_a_running_task() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = temp_registry(dir.path());
        let id = tasks.spawn("sleep 30", dir.path(), 1000).await.unwrap();
        let note = tasks.kill(id).await.unwrap();
        assert!(note.contains("kill signal sent"), "{note}");
        let exit = tasks.wait(id, Duration::from_secs(10)).await.unwrap();
        assert_eq!(exit.code, None, "killed → signal exit");
        let report = tasks.output(id).await.unwrap();
        assert!(report.contains("exited on a signal"), "{report}");
    }

    #[tokio::test]
    async fn unknown_task_ids_error_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = temp_registry(dir.path());
        assert!(tasks.output(99).await.is_err());
        assert!(tasks.kill(99).await.is_err());
    }
}
