//! Parent-side manager for background jobs.
//!
//! Each job runs in a fresh child process (this same executable in worker
//! mode), so heavy work can never freeze or crash the UI, its stderr output
//! is captured as live log lines, and it can be cancelled.

use crate::worker::{self, JobOp, JobSpec, WorkerResult};
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum JobEvent {
    Log(String),
    Done(JobResult),
}

#[derive(Debug, Clone)]
pub struct JobResult {
    pub ok: bool,
    pub cancelled: bool,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
}

pub struct RunningJob {
    child: Child,
    rx: Receiver<JobEvent>,
    tx: Sender<JobEvent>,
    started: Instant,
    pub op: JobOp,
}

impl RunningJob {
    /// Non-blocking poll for pending events.
    pub fn poll(&mut self) -> Option<JobEvent> {
        self.rx.try_recv().ok()
    }

    /// Kill the worker child. A `Done(cancelled)` event is queued so the UI
    /// settles the job on its next poll.
    pub fn cancel(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.tx.send(JobEvent::Done(JobResult {
            ok: false,
            cancelled: true,
            summary: "任务已取消（可能残留部分临时文件）".to_owned(),
            payload: None,
        }));
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

pub fn start(op: JobOp) -> Result<RunningJob> {
    let spec = JobSpec { op: op.clone() };
    let mut child = Command::new(std::env::current_exe().context("定位当前可执行文件")?)
        .env(worker::WORKER_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("启动后台工作进程")?;

    let mut stdin = child.stdin.take().context("无法获取工作进程的标准输入")?;
    let json = serde_json::to_string(&spec)?;
    stdin
        .write_all(json.as_bytes())
        .context("向工作进程发送任务")?;
    drop(stdin);

    let stdout = child.stdout.take().context("无法获取工作进程的标准输出")?;
    let stderr = child.stderr.take().context("无法获取工作进程的错误输出")?;

    let (tx, rx) = mpsc::channel::<JobEvent>();
    let tx_err = tx.clone();
    let tx_holder = tx.clone();

    // Protocol reader: stdout carries {"t":"log"|"result"} JSON lines.
    thread::spawn(move || {
        let mut saw_result = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match value.get("t").and_then(|t| t.as_str()) {
                Some("log") => {
                    if let Some(text) = value.get("s").and_then(|s| s.as_str()) {
                        let _ = tx.send(JobEvent::Log(text.to_owned()));
                    }
                }
                Some("result") => {
                    let result = serde_json::from_value::<WorkerResult>(value)
                        .map(|r| JobResult {
                            ok: r.ok,
                            cancelled: false,
                            summary: r.summary,
                            payload: r.payload,
                        })
                        .unwrap_or_else(|_| JobResult {
                            ok: false,
                            cancelled: false,
                            summary: "工作进程返回了无法解析的结果".to_owned(),
                            payload: None,
                        });
                    let _ = tx.send(JobEvent::Done(result));
                    saw_result = true;
                    break;
                }
                _ => {}
            }
        }
        if !saw_result {
            let _ = tx.send(JobEvent::Done(JobResult {
                ok: false,
                cancelled: false,
                summary: "工作进程异常退出，未返回结果".to_owned(),
                payload: None,
            }));
        }
    });

    // stderr forwarder: the library's progress output lands here verbatim.
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                let _ = tx_err.send(JobEvent::Log(trimmed.to_owned()));
            }
        }
    });

    Ok(RunningJob {
        child,
        rx,
        tx: tx_holder,
        started: Instant::now(),
        op,
    })
}
