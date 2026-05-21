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
/// `fifos` 字段：当端到端测试使用 FIFO 时，guard 在 drop 时一并 `unlink`，
/// 避免 `/tmp` 残留。空 vec 时跳过——上一阶段 `sleep 10` 用例不需要 FIFO。
/// 用 `Vec<PathBuf>` 而非 `Option<PathBuf>` 以支持「Reaping Before Each Prompt」
/// 阶段双 FIFO 场景（一个让 cat 退出，一个保持 cat 阻塞）。
struct Cleanup {
    shell: Option<Child>,
    fifos: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut shell) = self.shell.take() {
            let _ = shell.kill();
            let _ = shell.wait();
        }
        for path in self.fifos.drain(..) {
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
        fifos: Vec::new(),
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
        fifos: vec![fifo_path.clone()],
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
    // Stage「Reaping Before Each Prompt」契约：Done 行在窗口内**恰好出现一次**。
    // 自动 reap 路径与 `jobs` 内建任一先触发即渲染并移除，不得重复。
    // 不假设具体由哪条路径渲染：本阶段后通常由 prompt 前自动 reap 抢先渲染，
    // jobs2 看到空表无输出；但只要窗口内 Done 行恰好 1 次，契约即满足。
    let done_count = region2.matches("[1]+  Done").count()
        + region2.matches("[1]-  Done").count()
        + region2.matches("[1]   Done").count();
    assert_eq!(
        done_count, 1,
        "Done 行必须在 END1..END2 窗口内恰好出现一次（实测 {}）：\n{}",
        done_count, region2
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

// ---------------------------------------------------------------------------
// Stage「Reaping Before Each Prompt」端到端：
// 完成态作业在 prompt 前自动渲染 Done 行（夹在前一条命令输出与下一个 prompt 之间），
// 无需用户主动 `jobs`；marker 基于「Done + Running 全集」联合视图计算。
// ---------------------------------------------------------------------------

#[test]
fn done_appears_before_next_prompt() {
    // 端到端复刻题面示例 1 关键场景：
    //   $ cat <fifo1> &     # job 1
    //   $ cat <fifo2> &     # job 2
    //   # 让 job 1 退出（写 fifo1 + close 触发 EOF）
    //   $ echo BANANA
    //   BANANA
    //   [1]-  Done                    cat <fifo1>      ← 注意是 `-`，不是 `+`
    //   $
    //   $ jobs
    //   [2]+  Running                 cat <fifo2> &     ← 仅剩 job 2，marker 重算
    //
    // 验收点：
    // - BANANA 之后、下一个哨兵之前出现 `[1]-  Done` 子串（自动 reap 由 prompt 前路径渲染）
    // - marker 是 `-` 而非 `+`，因为彼时 job 2 仍 Running 是 last → 验证「联合视图」语义
    // - 后续 `jobs` 仅列 `[2]+  Running`，不再含 `[1]`（已被 reap 移除）

    // 0. 创建两个唯一 FIFO
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fifo1 = std::env::temp_dir().join(format!(
        "shell_reap_prompt_{}_{}_a.fifo",
        std::process::id(),
        now
    ));
    let fifo2 = std::env::temp_dir().join(format!(
        "shell_reap_prompt_{}_{}_b.fifo",
        std::process::id(),
        now
    ));
    for f in &[&fifo1, &fifo2] {
        let st = Command::new("mkfifo")
            .arg(f)
            .status()
            .expect("spawn mkfifo");
        assert!(st.success(), "mkfifo failed: {:?}", f);
    }

    // 1. spawn shell
    let bin = env!("CARGO_BIN_EXE_codecrafters-shell");
    let shell = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shell binary");

    let mut guard = Cleanup {
        shell: Some(shell),
        fifos: vec![fifo1.clone(), fifo2.clone()],
    };
    let shell = guard.shell.as_mut().unwrap();
    let mut shell_stdin = shell.stdin.take().expect("shell stdin");
    let shell_stdout = shell.stdout.take().expect("shell stdout");

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

    let f1 = fifo1.to_str().expect("fifo1 utf8");
    let f2 = fifo2.to_str().expect("fifo2 utf8");
    let mut acc = String::new();

    // ---- 第一步：双 `cat &` + 哨兵 END1，确认两个作业都已 Running ----
    shell_stdin
        .write_all(format!("cat {} &\n", f1).as_bytes())
        .expect("write cat fifo1");
    thread::sleep(Duration::from_millis(150));
    shell_stdin
        .write_all(format!("cat {} &\n", f2).as_bytes())
        .expect("write cat fifo2");
    thread::sleep(Duration::from_millis(150));
    shell_stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel 1");
    shell_stdin.flush().expect("flush 1");

    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(5));

    // ---- 第二步：让 job 1 退出（write+close fifo1）→ 等 reap → 喂 echo BANANA + 哨兵 END2 ----
    {
        let _w = std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo1)
            .expect("open fifo1 for write");
        // _w drop 触发对端 cat 的 read 返回 0 → EOF → 干净退出
    }
    // 等 cat 退出
    thread::sleep(Duration::from_millis(400));

    shell_stdin.write_all(b"echo BANANA\n").expect("write echo");
    shell_stdin
        .write_all(b"echo END_SENTINEL_2\n")
        .expect("write sentinel 2");
    shell_stdin.flush().expect("flush 2");

    drain_until(&rx, &mut acc, &["END_SENTINEL_2"], Duration::from_secs(5));

    // 第二步窗口：END1..END2 之间应包含 BANANA、随后 `[1]-  Done` 自动 reap 行。
    // 关键：marker 是 `-`（非 `+`），因为 job 2 仍 Running 是 last → 验证联合视图。
    let region2 = {
        let start = acc.find("END_SENTINEL_1").unwrap_or(0);
        let end = acc.find("END_SENTINEL_2").unwrap_or(acc.len());
        acc[start..end].to_string()
    };
    assert!(
        region2.contains("BANANA"),
        "窗口内必须包含 BANANA 输出；窗口:\n{}",
        region2
    );
    assert!(
        region2.contains("[1]-  Done"),
        "BANANA 之后必须自动出现 `[1]-  Done`（marker 必须是 `-`，非 `+`，验证联合视图）；窗口:\n{}",
        region2
    );
    // BANANA 在 Done 之前（自动 reap 发生在下一轮 prompt 前，即 BANANA 输出之后）
    let pos_banana = region2.find("BANANA").expect("BANANA pos");
    let pos_done = region2.find("[1]-  Done").expect("Done pos");
    assert!(
        pos_banana < pos_done,
        "BANANA 必须出现在 Done 行之前；窗口:\n{}",
        region2
    );
    // Done 行不带尾 ` &`
    let done_line = region2
        .lines()
        .find(|l| l.contains("[1]-  Done"))
        .expect("locate Done line");
    assert!(
        !done_line.ends_with(" &"),
        "Done 行不得以 ` &` 结尾：`{}`",
        done_line
    );
    // Done 命令字段含 fifo1 片段
    assert!(
        done_line.contains(f1),
        "Done 行命令字段应含 fifo1 路径 `{}`；行: `{}`",
        f1,
        done_line
    );

    // ---- 第三步：jobs + 哨兵 END3，期望仅剩 [2]+  Running，无 [1] ----
    shell_stdin.write_all(b"jobs\n").expect("write jobs final");
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
    // job 2 仍 Running，marker 重算为 `+`（reap 后仅它一项）
    assert!(
        region3.contains("[2]+  Running"),
        "jobs 窗口必须含 `[2]+  Running`（marker 重算为 `+`）；窗口:\n{}",
        region3
    );
    // [1] 已被 reap 移除，不得再现（任何 mark 形态）
    assert!(
        !region3.contains("[1]"),
        "jobs 窗口不得再包含 `[1]`（已自动 reap 移除）；窗口:\n{}",
        region3
    );

    // 让 job 2 也退出，避免 shell 退出后 cat 残留
    {
        let _w = std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo2)
            .expect("open fifo2 for write");
    }

    drop(shell_stdin);
    drop(guard);
}

// ---------------------------------------------------------------------------
// Stage「Recycling Job Numbers」端到端：作业编号回收（最小可用正整数分配）。
// 复刻 codecrafters tester 的两条核心流程：
//   - 流程 A：作业表清空后下个 `&` 命令必须从 [1] 重新开始；
//   - 流程 B：单条作业完成 + 自动 reap 后下个 `&` 命令复用刚释放的编号。
// ---------------------------------------------------------------------------

#[test]
fn recycle_to_one_when_empty() {
    // tester 流程 A 端到端复刻：
    //   $ cat <fifo> &        → [1] <pid>
    //   # 写 fifo + EOF 让 cat 退出
    //   $ echo apple          → "apple\n" + 自动 reap 渲染 [1]+ Done
    //   $ sleep 100 &         → 必须打印 [1] <pid>（编号回收为 1，不是 2）
    //   $ jobs                → [1]+  Running                 sleep 100 &
    //
    // 关键断言：
    // - `sleep 100 &` 之后、下一个哨兵之前，窗口内必须出现 `[1] ` 通知子串；
    // - 后续 `jobs` 窗口必须含 `[1]+  Running` + `sleep 100`。
    //
    // 用哨兵 `END_SENTINEL_<N>` 切窗口，避免最初 `cat <fifo> &` 的 `[1]` 通知
    // 干扰对「回收后新通知」的断言——只检查 END_SENTINEL_2..END_SENTINEL_3 区间。

    // 0. 创建唯一 FIFO
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fifo_path = std::env::temp_dir().join(format!(
        "shell_recycle_empty_{}_{}.fifo",
        std::process::id(),
        now
    ));
    let mkfifo_status = Command::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .expect("spawn mkfifo");
    assert!(mkfifo_status.success(), "mkfifo failed: {:?}", fifo_path);

    // 1. spawn shell
    let bin = env!("CARGO_BIN_EXE_codecrafters-shell");
    let shell = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shell binary");

    let mut guard = Cleanup {
        shell: Some(shell),
        fifos: vec![fifo_path.clone()],
    };
    let shell = guard.shell.as_mut().unwrap();
    let mut shell_stdin = shell.stdin.take().expect("shell stdin");
    let shell_stdout = shell.stdout.take().expect("shell stdout");

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

    // ---- 步骤 1：cat <fifo> & 启动 [1] + 哨兵 END1 ----
    shell_stdin
        .write_all(format!("cat {} &\n", fifo_str).as_bytes())
        .expect("write cat bg");
    thread::sleep(Duration::from_millis(150));
    shell_stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel 1");
    shell_stdin.flush().expect("flush 1");
    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(5));

    // ---- 步骤 2：写 fifo + close 让 cat 退出 + 等 reap ----
    {
        let _w = std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo_path)
            .expect("open fifo for write");
    }
    thread::sleep(Duration::from_millis(400));

    // 喂 echo apple → 触发下一轮 prompt 前自动 reap 渲染 `[1]+ Done` + 哨兵 END2。
    // END2 之后表已空，下一条 `sleep 100 &` 必须分配编号 1。
    shell_stdin.write_all(b"echo apple\n").expect("write echo");
    shell_stdin
        .write_all(b"echo END_SENTINEL_2\n")
        .expect("write sentinel 2");
    shell_stdin.flush().expect("flush 2");
    drain_until(&rx, &mut acc, &["END_SENTINEL_2"], Duration::from_secs(5));

    // ---- 步骤 3：表空 → sleep 100 & 必须打印 `[1] <pid>` ----
    shell_stdin
        .write_all(b"sleep 100 &\n")
        .expect("write sleep bg");
    thread::sleep(Duration::from_millis(150));
    shell_stdin.write_all(b"jobs\n").expect("write jobs");
    shell_stdin
        .write_all(b"echo END_SENTINEL_3\n")
        .expect("write sentinel 3");
    shell_stdin.flush().expect("flush 3");
    drain_until(&rx, &mut acc, &["END_SENTINEL_3"], Duration::from_secs(5));

    // 第三步窗口：END2..END3 之间必须出现回收后的 `[1] ` 通知 + jobs `[1]+  Running`
    let region3 = {
        let start = acc.find("END_SENTINEL_2").unwrap_or(0);
        let end = acc.find("END_SENTINEL_3").unwrap_or(acc.len());
        acc[start..end].to_string()
    };
    // 通知行：`[1] <pid>`（数字 + 空格 + pid）。用 `[1] ` 子串足以排他匹配——
    // jobs 行格式是 `[1]+  ` 或 `[1]-  `（mark 后必有 2 空格），二者 `[1] ` 后
    // 紧跟数字 PID，与 jobs 行的 mark 字符 `+`/`-`/` ` 不同。但通知行后跟数字
    // PID，jobs 行后跟空白 mark 也可能匹配 `[1] `——用更精确判定：通知行起始
    // 于行首 `[1] ` 后接数字。
    let has_notify = region3.lines().any(|l| {
        l.strip_prefix("[1] ")
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty())
    });
    assert!(
        has_notify,
        "sleep 100 & 必须打印 `[1] <pid>` 通知（编号回收为 1）；窗口:\n{}",
        region3
    );
    assert!(
        region3.contains("[1]+  Running") && region3.contains("sleep 100"),
        "jobs 窗口必须含 `[1]+  Running` + `sleep 100`；窗口:\n{}",
        region3
    );

    drop(shell_stdin);
    drop(guard);
}

#[test]
fn reuse_two_with_one_remaining() {
    // tester 流程 B 端到端复刻：
    //   $ sleep 100 &         → [1] <pid>
    //   $ cat <fifo> &        → [2] <pid>
    //   # 写 fifo + EOF 让 cat 退出
    //   $ echo word           → "word\n" + 自动 reap 渲染 [2]+ Done
    //   # 表只剩 [1] sleep 100；最小可用是 2
    //   $ sleep 50 &          → 必须打印 [2] <pid>（编号回收为 2，不是 3）
    //   $ jobs                → [1]-  Running                 sleep 100 &
    //                          [2]+  Running                 sleep 50 &
    //
    // 关键断言：
    // - 步骤 3 窗口内必须出现 `[2] <pid>` 通知（编号回收）；
    // - jobs 窗口必须同时含 `[1]-  Running` + `sleep 100` 与 `[2]+  Running` + `sleep 50`。

    // 0. 创建唯一 FIFO
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fifo_path = std::env::temp_dir().join(format!(
        "shell_recycle_two_{}_{}.fifo",
        std::process::id(),
        now
    ));
    let mkfifo_status = Command::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .expect("spawn mkfifo");
    assert!(mkfifo_status.success(), "mkfifo failed: {:?}", fifo_path);

    // 1. spawn shell
    let bin = env!("CARGO_BIN_EXE_codecrafters-shell");
    let shell = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shell binary");

    let mut guard = Cleanup {
        shell: Some(shell),
        fifos: vec![fifo_path.clone()],
    };
    let shell = guard.shell.as_mut().unwrap();
    let mut shell_stdin = shell.stdin.take().expect("shell stdin");
    let shell_stdout = shell.stdout.take().expect("shell stdout");

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

    // ---- 步骤 1：sleep 100 & ([1]) + cat <fifo> & ([2]) + 哨兵 END1 ----
    shell_stdin
        .write_all(b"sleep 100 &\n")
        .expect("write sleep bg");
    thread::sleep(Duration::from_millis(150));
    shell_stdin
        .write_all(format!("cat {} &\n", fifo_str).as_bytes())
        .expect("write cat bg");
    thread::sleep(Duration::from_millis(150));
    shell_stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel 1");
    shell_stdin.flush().expect("flush 1");
    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(5));

    // ---- 步骤 2：让 [2] cat 退出 + 等自动 reap，再 echo word + 哨兵 END2 ----
    {
        let _w = std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo_path)
            .expect("open fifo for write");
    }
    thread::sleep(Duration::from_millis(400));

    shell_stdin.write_all(b"echo word\n").expect("write echo");
    shell_stdin
        .write_all(b"echo END_SENTINEL_2\n")
        .expect("write sentinel 2");
    shell_stdin.flush().expect("flush 2");
    drain_until(&rx, &mut acc, &["END_SENTINEL_2"], Duration::from_secs(5));

    // ---- 步骤 3：表只剩 [1]，下条 sleep 50 & 必须分配编号 2 ----
    shell_stdin
        .write_all(b"sleep 50 &\n")
        .expect("write sleep 50 bg");
    thread::sleep(Duration::from_millis(150));
    shell_stdin.write_all(b"jobs\n").expect("write jobs");
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

    // 步骤 3 窗口必须出现回收后的 `[2] <pid>` 通知（行首 `[2] ` + 数字 PID）
    let has_notify = region3.lines().any(|l| {
        l.strip_prefix("[2] ")
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty())
    });
    assert!(
        has_notify,
        "sleep 50 & 必须打印 `[2] <pid>` 通知（编号回收为 2，非 3）；窗口:\n{}",
        region3
    );
    // jobs 输出：[1] 是 last-1（`-`）、[2] 是 last（`+`）；marker 算法基于 Vec 索引，
    // 与既有 `jobs_mixed_running_and_done_retain_only_running` 单测语义一致。
    assert!(
        region3.contains("[1]-  Running") && region3.contains("sleep 100"),
        "jobs 窗口必须含 `[1]-  Running` + `sleep 100`；窗口:\n{}",
        region3
    );
    assert!(
        region3.contains("[2]+  Running") && region3.contains("sleep 50"),
        "jobs 窗口必须含 `[2]+  Running` + `sleep 50`；窗口:\n{}",
        region3
    );

    drop(shell_stdin);
    drop(guard);
}
