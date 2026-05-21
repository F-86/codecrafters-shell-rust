//! 集成测试共享辅助工具。
//!
//! 把 `Cleanup` RAII guard、`drain_until` 阻塞读、`spawn_shell` 启动辅助等
//! 跨文件复用的能力抽到这里，避免 `tests/*.rs` 之间复制粘贴漂移。
//!
//! 用 `tests/common/mod.rs` 而非 `tests/common.rs` 的原因：cargo 把 `tests/`
//! 下每个 `.rs` 文件当作独立 integration crate 编译；`mod.rs` 子目录形式被
//! cargo 跳过编译为 crate，仅当被 `mod common;` 显式引入时作为模块加入对应
//! crate——这是 Rust 官方推荐的共享 helpers 组织方式。
//!
//! 现有 `jobs_builtin.rs` / `background_stdio.rs` 早于本辅助实现，仍维持内联
//! 工具风格；pipeline 系列测试 (`pipeline_basic.rs` / `pipeline_builtin.rs`)
//! 起统一引用本模块。

#![cfg(unix)]
#![allow(dead_code)]

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// RAII 清理：测试 panic / 正常结束都会执行：
/// 1. kill + wait shell 子进程，防止僵尸 + 释放 FIFO 读端句柄；
/// 2. unlink 全部临时 FIFO / 文件，避免 `/tmp` 残留。
pub struct Cleanup {
    pub shell: Option<Child>,
    pub temp_paths: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut shell) = self.shell.take() {
            let _ = shell.kill();
            let _ = shell.wait();
        }
        for path in self.temp_paths.drain(..) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// spawn 编译产物 shell 二进制并接管三管道。
///
/// 返回：(Cleanup guard, stdin, channel-receiver-of-stdout-chunks)
/// stderr 仅 piped 但不读——避免 stderr 阻塞，目前所有 pipeline 测试只断言 stdout。
pub fn spawn_shell() -> (Cleanup, ChildStdin, mpsc::Receiver<Vec<u8>>) {
    let bin = env!("CARGO_BIN_EXE_codecrafters-shell");
    let mut shell = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shell binary");

    let stdin = shell.stdin.take().expect("shell stdin");
    let stdout = shell.stdout.take().expect("shell stdout");

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    spawn_reader_thread(stdout, tx);

    let guard = Cleanup {
        shell: Some(shell),
        temp_paths: Vec::new(),
    };
    (guard, stdin, rx)
}

/// 后台线程持续 read 一个 stdout 句柄并把 chunk 通过 mpsc::channel 发送，
/// 直到 EOF 或对端 receiver 被 drop。
fn spawn_reader_thread(mut reader: ChildStdout, tx: mpsc::Sender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// 阻塞读 mpsc 直到累积字符串包含全部 needles，或 deadline 超时。
/// 即便超时也返回当前累积内容，由调用方自行断言（便于打印诊断信息）。
pub fn drain_until(
    rx: &mpsc::Receiver<Vec<u8>>,
    acc: &mut String,
    needles: &[&str],
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if needles.iter().all(|n| acc.contains(n)) {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining.max(Duration::from_millis(20))) {
            Ok(chunk) => acc.push_str(&String::from_utf8_lossy(&chunk)),
            Err(mpsc::RecvTimeoutError::Timeout) => return,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// 在 `/tmp` 下创建唯一路径（不创建文件本身），用 PID + 纳秒避免并发碰撞。
///
/// 用于 FIFO / 普通文件路径预分配——调用方负责实际 `mkfifo` / `File::create`，
/// 并把返回的 PathBuf push 进 `Cleanup.temp_paths` 以保证清理。
pub fn unique_tmp_path(prefix: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "shell_{}_{}_{}.tmp",
        prefix,
        std::process::id(),
        now
    ))
}

/// 用系统 `mkfifo` 在给定路径上创建 FIFO；失败 panic。
///
/// 选用 coreutils mkfifo 而非 `nix::unistd::mkfifo` 是为了零新依赖——与
/// `your_program.sh` 共享同一工具链。
pub fn create_fifo(path: &PathBuf) {
    let st = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("spawn mkfifo");
    assert!(st.success(), "mkfifo failed: {:?}", path);
}
