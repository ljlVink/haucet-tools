use crate::worker::{self, JobOp, JobSpec, WorkerResult};
use anyhow::{Context, Result};
use common::process::hide_command_window;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    _tx: Sender<JobEvent>,
    cancelled: Arc<AtomicBool>,
    started: Instant,
    pub op: JobOp,
}

impl RunningJob {
    /// Non-blocking poll for pending events.
    pub fn poll(&mut self) -> Option<JobEvent> {
        self.rx.try_recv().ok()
    }

    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

pub fn start(op: JobOp) -> Result<RunningJob> {
    let spec = JobSpec { op: op.clone() };
    let mut command = Command::new(std::env::current_exe().context("定位当前可执行文件")?);
    hide_command_window(&mut command);
    let mut child = command
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
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_reader = cancelled.clone();

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
            let cancelled = cancelled_reader.load(Ordering::SeqCst);
            let _ = tx.send(JobEvent::Done(JobResult {
                ok: false,
                cancelled,
                summary: if cancelled {
                    "任务已取消（可能残留部分临时文件）".to_owned()
                } else {
                    "工作进程异常退出, 未返回结果".to_owned()
                },
                payload: None,
            }));
        }
    });

    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let cleaned = crate::util::strip_ansi(&line);
            let trimmed = cleaned.trim_end();
            if !trimmed.is_empty() {
                let _ = tx_err.send(JobEvent::Log(trimmed.to_owned()));
            }
        }
    });

    Ok(RunningJob {
        child,
        rx,
        _tx: tx_holder,
        cancelled,
        started: Instant::now(),
        op,
    })
}
