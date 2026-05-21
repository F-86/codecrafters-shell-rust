//! 集成测试：codecrafters「list a single background job」stage 端到端验证。
//!
//! 复刻 codecrafters tester 的执行序列：
//!
//! ```text
//! $ ./your_program.sh
//! $ sleep 10 &
//! [1] <pid>
//! $ jobs
//! [1]+  Running                 sleep 10 &
//! ```
//!
//! 断言点：
//! - 输出中必须包含 `[1]+  Running` 子串（验证编号 / `+` 标记 / 状态字段）
//! - 输出中必须包含 `sleep 10` 子串（验证命令字符串；尾 `&` 可选，tester 容忍）
//!
//! 设计要点与 `background_stdio.rs` 一致：
//! - **不引入新依赖**：通过管道喂 stdin、读 stdout；超时由 `mpsc::recv_timeout` 实现。
//! - **测试用 `sleep 10`** 而非 cat fifo：sleep 不阻塞读端、不需要 FIFO，更纯粹。
//!   shell 启动 sleep 后台后立即返回提示符，给 `jobs` 命令足够窗口列出 Running。
//! - **断言子串而非整行**：rustyline 在非 tty 下仍打 prompt（`$ `），会与输出混排；
//!   只检查关键 token 出现即可，与 codecrafters tester 自身的子串校验风格一致。
//! - **清理**：RAII guard 兜底 kill shell + wait，确保 sleep 子进程不残留。

#![cfg(unix)]

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// RAII 清理保证：测试 panic / 正常结束都会执行。
struct Cleanup {
    shell: Option<Child>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut shell) = self.shell.take() {
            let _ = shell.kill();
            let _ = shell.wait();
        }
    }
}

#[test]
fn jobs_lists_single_running_background_job() {
    // 1. spawn shell 二进制，三管道接管
    let bin = env!("CARGO_BIN_EXE_codecrafters-shell");
    let shell = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shell binary");

    let mut guard = Cleanup {
        shell: Some(shell),
    };
    let shell = guard.shell.as_mut().unwrap();

    let mut shell_stdin = shell.stdin.take().expect("shell stdin");
    let shell_stdout = shell.stdout.take().expect("shell stdout");

    // 2. 后台线程持续 read shell stdout 累积成 chunk 流
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut reader = shell_stdout;
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

    // 3. 给 shell 喂命令：先后台 sleep 10，再 jobs 列出。
    //    sleep 10 在 PATH 中 (coreutils)，不需要任何 FIFO 或 IO 同步。
    shell_stdin
        .write_all(b"sleep 10 &\n")
        .expect("write sleep cmd");
    // 留 200ms 让 spawn 完成、`[1] PID` 通知行写到 shell stdout，
    // 并让作业表 push 完毕；时间窗口给 jobs 列出做准备。
    thread::sleep(Duration::from_millis(200));
    shell_stdin.write_all(b"jobs\n").expect("write jobs cmd");
    shell_stdin.flush().expect("flush stdin");

    // 4. 累积 shell stdout，直到看到 `[1]+  Running` 子串或 5s 超时
    let mut acc = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(remaining.max(Duration::from_millis(50))) {
            Ok(chunk) => {
                acc.push_str(&String::from_utf8_lossy(&chunk));
                // `[1]+  Running` + `sleep 10` 两个 token 都出现即可停
                if acc.contains("[1]+  Running") && acc.contains("sleep 10") {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // 5. 关闭 shell stdin → 触发 readline EOF → shell 主循环 break 退出
    //    （`Cleanup::drop` 再兜底 kill，但正常路径下 shell 自行干净退出更优）
    drop(shell_stdin);

    // 6. 断言：tester 验证的 4 个要点
    //    - `[1]` job 编号
    //    - `+` 最近作业标记
    //    - `Running` 状态
    //    - 命令含 `sleep 10`（尾 `&` 可选）
    let ok_marker = acc.contains("[1]+  Running");
    let ok_cmd = acc.contains("sleep 10");
    if !(ok_marker && ok_cmd) {
        eprintln!(
            "---- shell accumulated stdout ----\n{}\n---- end ----",
            acc
        );
    }
    assert!(
        ok_marker,
        "jobs 输出必须包含 `[1]+  Running` 子串（编号/标记/状态字段）"
    );
    assert!(ok_cmd, "jobs 输出必须包含 `sleep 10` 命令子串");

    drop(guard);
}
