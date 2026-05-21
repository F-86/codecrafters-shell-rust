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
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// RAII 清理保证：测试 panic / 正常结束都会执行。
///
/// `fifo` 字段：当 Stage「Manage Jobs」端到端测试使用 FIFO 时，guard 在 drop 时
/// 一并 `unlink`，避免 `/tmp` 残留。`None` 时跳过——上一阶段 `sleep 10` 用例不需要 FIFO。
struct Cleanup {
    shell: Option<Child>,
    fifo: Option<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut shell) = self.shell.take() {
            let _ = shell.kill();
            let _ = shell.wait();
        }
        if let Some(path) = self.fifo.take() {
            let _ = std::fs::remove_file(path);
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
        fifo: None,
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

// ---------------------------------------------------------------------------
// Stage「Manage Jobs」端到端：完成态作业被 reap 后 `jobs` 列出 Done 一次并移除。
// ---------------------------------------------------------------------------

/// 阻塞读 shell stdout 直到累积字符串包含全部 needles，或 deadline 超时。
/// 返回累积到的字符串。即便超时也返回当前累积内容，由调用方自行断言。
fn drain_until(
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

#[test]
fn jobs_done_then_removed() {
    // 端到端三步序列：
    //   1. `cat <fifo> &`  →  `jobs1` 期望 [1]+  Running                cat <fifo> &
    //   2. 写空到 fifo（open RW + close 触发 cat 读 EOF 退出）+ 等 reap
    //      然后 `jobs2` 期望 [1]+  Done                    cat <fifo>
    //   3. `jobs3 SENT3` 哨兵 → 期望哨兵之前不再有 `[1]` 作业行
    //
    // 设计要点：
    // - **FIFO 而非 sleep**：必须能让子进程「按需退出」以观察 Running→Done 状态推进。
    //   sleep 10 不便提前唤醒；用 FIFO 让 cat 阻塞读，由测试侧 open+close
    //   触发对端 EOF 让 cat 干净退出。
    // - **mkfifo 命令**：用 `Command::new("mkfifo")` 而非 `nix::unistd::mkfifo`
    //   以避免引入新依赖，与 `your_program.sh` 同一个 coreutils 工具链。
    // - **打开 fifo 的方式**：以 RDWR 风格——但 std 不直接暴露 O_RDWR fopen，
    //   故用 `OpenOptions::write(true).open(...)` 并立即 drop 即可：cat 对端
    //   read 到 0 字节 EOF 退出。fifo 可被多次打开/关闭，无副作用。
    // - **节奏**：写完 fifo 后 sleep 400ms，给 cat 退出 + 给 shell 在下一行
    //   readline 之前的 prompt-前 reap 推进一次。
    // - **哨兵 `END_SENTINEL_<N>`**：保证我们 drain 到的输出已穿过对应那次
    //   `jobs` 的全部行，避免 ringbuffer 边界误判。

    // 0. 创建唯一 FIFO 路径：用 PID + 纳秒时间戳避免并发测试碰撞。
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fifo_path = std::env::temp_dir().join(format!(
        "shell_jobs_done_{}_{}.fifo",
        std::process::id(),
        now
    ));
    let mkfifo_status = Command::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .expect("spawn mkfifo");
    assert!(mkfifo_status.success(), "mkfifo failed: {:?}", fifo_path);

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
        fifo: Some(fifo_path.clone()),
    };
    let shell = guard.shell.as_mut().unwrap();
    let mut shell_stdin = shell.stdin.take().expect("shell stdin");
    let shell_stdout = shell.stdout.take().expect("shell stdout");

    // 后台读线程，累积 stdout chunk
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

    let fifo_str = fifo_path.to_str().expect("fifo path utf8");
    let mut acc = String::new();

    // ---- 第一步：cat <fifo> & + jobs1 + 哨兵 END1 ----
    // cat 在 FIFO 上阻塞读，进入 Running。
    shell_stdin
        .write_all(format!("cat {} &\n", fifo_str).as_bytes())
        .expect("write cat bg");
    // 给 spawn + 通知行 + push job 留时间，然后 jobs。
    thread::sleep(Duration::from_millis(200));
    shell_stdin.write_all(b"jobs\n").expect("write jobs1");
    shell_stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel 1");
    shell_stdin.flush().expect("flush 1");

    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(5));
    let acc_after_jobs1 = acc.clone();

    // 第一步断言：Running 行存在
    assert!(
        acc_after_jobs1.contains("[1]+  Running"),
        "jobs1 必须含 `[1]+  Running`；累积输出:\n{}",
        acc_after_jobs1
    );
    assert!(
        acc_after_jobs1.contains("cat ") && acc_after_jobs1.contains(" &"),
        "jobs1 Running 行必须包含 `cat ` 与尾 ` &`；累积输出:\n{}",
        acc_after_jobs1
    );

    // ---- 第二步：让 cat 退出 + 等 reap 推进，再 jobs2 + 哨兵 END2 ----
    // 以 write 模式 open 然后立即 close FIFO：对端 cat 的 read 返回 0 → EOF → 干净退出。
    {
        let _w = std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo_path)
            .expect("open fifo for write");
        // _w 在作用域结束时关闭，cat 收到 EOF
    }
    // 给 cat 退出 + 让 shell 在下一行 readline 前的 prompt-前 reap 推进
    thread::sleep(Duration::from_millis(500));

    shell_stdin.write_all(b"jobs\n").expect("write jobs2");
    shell_stdin
        .write_all(b"echo END_SENTINEL_2\n")
        .expect("write sentinel 2");
    shell_stdin.flush().expect("flush 2");

    drain_until(&rx, &mut acc, &["END_SENTINEL_2"], Duration::from_secs(5));
    // 第二步只关心 END1..END2 之间新增片段
    let region2 = {
        let start = acc.find("END_SENTINEL_1").unwrap_or(0);
        let end = acc.find("END_SENTINEL_2").unwrap_or(acc.len());
        acc[start..end].to_string()
    };
    assert!(
        region2.contains("[1]+  Done"),
        "jobs2 必须含 `[1]+  Done`；窗口输出:\n{}",
        region2
    );
    // Done 行不应有尾 ` &`：检查 Done 所在那一行
    let done_line = region2
        .lines()
        .find(|l| l.contains("[1]+  Done"))
        .expect("find Done line");
    assert!(
        !done_line.ends_with(" &"),
        "Done 行不得以 ` &` 结尾：`{}`",
        done_line
    );

    // ---- 第三步：jobs3 + 哨兵 END3，期望窗口内不再含 `[1]` 作业行 ----
    shell_stdin.write_all(b"jobs\n").expect("write jobs3");
    shell_stdin
        .write_all(b"echo END_SENTINEL_3\n")
        .expect("write sentinel 3");
    shell_stdin.flush().expect("flush 3");

    drain_until(&rx, &mut acc, &["END_SENTINEL_3"], Duration::from_secs(5));
    let region3 = {
        let start = acc.find("END_SENTINEL_2").unwrap_or(0);
        let end = acc.find("END_SENTINEL_3").unwrap_or(acc.len());
        acc[start..end].to_string()
    };
    // 窗口内不应再出现 `[1]` 这种作业行（包括 Running / Done / 任何 mark）
    assert!(
        !region3.contains("[1]"),
        "jobs3 窗口内不得再包含 `[1]` 作业行；窗口输出:\n{}",
        region3
    );

    // 4. 干净退出 shell
    drop(shell_stdin);
    drop(guard);
}
