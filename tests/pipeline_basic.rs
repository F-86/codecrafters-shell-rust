//! 集成测试：pipeline 基础能力端到端验证。
//!
//! 覆盖场景：
//! - 两段外部命令 `cat file | wc`：题面 codecrafters「Pipelines」stage 验收路径；
//! - 三段 pipeline `cat | head | wc -l`：验证 N 段串联（N>2）；
//! - 段内重定向 `cat file | wc -l > out.txt`：验证 pipeline 与 `>` 共存；
//! - `tail -f | head -n 5` SIGPIPE 自然回收：题面关键路径，head 满 5 行后整条
//!   pipeline 必须能自动退出，不卡死；
//! - pipeline 中命令未找到：`cat file | nosuchcmd` 报 not found，shell 不崩。
//!
//! 设计要点（与 `jobs_builtin.rs` 一致）：
//! - **不引入新依赖**：复用 std + mpsc + 共享 `tests/common`；
//! - **每条命令用 `END_SENTINEL_<N>` 哨兵切窗口**，避免输出穿插；
//! - **timeout 防卡死**：每次 `drain_until` 限定 5s 上限，SIGPIPE 用例额外断言
//!   整体 wait 在 timeout 之内完成；
//! - **断言子串而非整行**：rustyline 非 tty 下仍打 prompt（`$ `）会与输出混排。

#![cfg(unix)]

mod common;

use std::io::Write;
use std::time::Duration;

use common::{create_fifo, drain_until, spawn_shell, unique_tmp_path, Cleanup};

/// 在 `/tmp` 下创建一个 N 行内容的临时文件，返回路径（自动加入 cleanup 列表）。
fn make_temp_file(guard: &mut Cleanup, prefix: &str, lines: usize) -> std::path::PathBuf {
    let path = unique_tmp_path(prefix);
    let mut f = std::fs::File::create(&path).expect("create temp file");
    for i in 1..=lines {
        writeln!(f, "line{}", i).expect("write temp line");
    }
    drop(f);
    guard.temp_paths.push(path.clone());
    path
}

#[test]
fn two_stage_cat_pipe_wc_lines() {
    // 题面验收场景：`cat /tmp/foo/file | wc -l` 应正确输出文件行数。
    // 用临时文件避免依赖 `/tmp/foo/`，避免与 codecrafters tester 共享状态。

    let (mut guard, mut stdin, rx) = spawn_shell();
    let file = make_temp_file(&mut guard, "pipe_cat_wc", 7);
    let file_str = file.to_str().expect("path utf8");

    // 命令 + 哨兵切窗口
    stdin
        .write_all(format!("cat {} | wc -l\n", file_str).as_bytes())
        .expect("write pipe cmd");
    stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel");
    stdin.flush().expect("flush");

    let mut acc = String::new();
    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(5));

    // 取 END_SENTINEL_1 之前的窗口
    let window = match acc.find("END_SENTINEL_1") {
        Some(idx) => &acc[..idx],
        None => acc.as_str(),
    };
    // wc -l 输出形如 "      7" 或 "7"（不同 GNU/BSD 行为）——只要某行 trim 后等于 7。
    let found = window.lines().any(|l| l.trim() == "7");
    assert!(
        found,
        "`cat | wc -l` 必须输出 7（文件行数）；累积窗口:\n{}",
        window
    );

    drop(stdin);
    drop(guard);
}

#[test]
fn three_stage_pipeline_head_then_wc() {
    // 三段 pipeline：`cat file | head -n 3 | wc -l` 应输出 3。
    // 验证 N 段串联（中间段 head 既读上游又写下游，pipe fd 链式 dup2 正确性）。
    let (mut guard, mut stdin, rx) = spawn_shell();
    let file = make_temp_file(&mut guard, "pipe_three", 10);
    let file_str = file.to_str().expect("path utf8");

    stdin
        .write_all(format!("cat {} | head -n 3 | wc -l\n", file_str).as_bytes())
        .expect("write three-stage pipe");
    stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel");
    stdin.flush().expect("flush");

    let mut acc = String::new();
    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(5));

    let window = match acc.find("END_SENTINEL_1") {
        Some(idx) => &acc[..idx],
        None => acc.as_str(),
    };
    let found = window.lines().any(|l| l.trim() == "3");
    assert!(
        found,
        "三段 pipeline 必须输出 3（head 截取后 wc -l 计数）；窗口:\n{}",
        window
    );

    drop(stdin);
    drop(guard);
}

#[test]
fn pipeline_with_last_stage_stdout_redirect() {
    // pipeline 末段段内 `>` 重定向：`cat file | wc -l > out.txt` 把计数写到文件。
    // 验证段内重定向优先级高于 pipe stdout：末段 wc -l 的 stdout 应走文件而非终端。
    let (mut guard, mut stdin, rx) = spawn_shell();
    let file = make_temp_file(&mut guard, "pipe_redirect_in", 4);
    let file_str = file.to_str().expect("path utf8");
    let out_path = unique_tmp_path("pipe_redirect_out");
    guard.temp_paths.push(out_path.clone());
    let out_str = out_path.to_str().expect("path utf8");

    stdin
        .write_all(format!("cat {} | wc -l > {}\n", file_str, out_str).as_bytes())
        .expect("write redirect pipe");
    stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel");
    stdin.flush().expect("flush");

    let mut acc = String::new();
    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(5));

    // 读取重定向目标文件，断言内容是 4
    let content = std::fs::read_to_string(&out_path).expect("read out file");
    assert!(
        content.lines().any(|l| l.trim() == "4"),
        "段内 `>` 必须把 wc -l 输出（4）写到 out 文件；实际内容:\n{}",
        content
    );

    drop(stdin);
    drop(guard);
}

#[test]
fn tail_f_pipe_head_sigpipe_terminates() {
    // 题面关键路径：`tail -f <fifo> | head -n 5`。
    // - tail -f 持续追加输出；
    // - head -n 5 收齐 5 行后 close stdin → tail 下次 write 收 SIGPIPE 默认终止；
    // - shell wait 全部子进程，整条 pipeline 必须在 timeout 内退出，不卡死。
    //
    // 测试构造：用临时 FIFO 而非临时文件——tail -f 对普通文件用 inotify，
    // tester 上的内核可能没启用；用 FIFO 时 tail -f 直接走 read 阻塞，更可控。
    // 写入侧由测试线程同步推 5 行 + 关闭 fifo 写端。

    let (mut guard, mut stdin, rx) = spawn_shell();
    let fifo_path = unique_tmp_path("pipe_tail_head");
    create_fifo(&fifo_path);
    guard.temp_paths.push(fifo_path.clone());
    let fifo_str = fifo_path.to_str().expect("path utf8");

    // 启动 pipeline（tail -f 在 fifo 上阻塞读）+ 哨兵
    // 注：tail -f 对 FIFO 与普通文件略有差异——`-f` 在 FIFO 上等价于持续 read。
    stdin
        .write_all(format!("tail -f {} | head -n 5\n", fifo_str).as_bytes())
        .expect("write tail|head");
    stdin.flush().expect("flush");

    // 给 shell 与子进程 spawn 一点时间
    std::thread::sleep(Duration::from_millis(200));

    // 测试侧向 fifo 写 5 行 + 关闭，让 head 收齐 5 行后退出
    {
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo_path)
            .expect("open fifo for write");
        for i in 1..=10 {
            // 写超过 5 行，确保 head 收齐 5 行触发 SIGPIPE 路径而非 EOF 路径
            let _ = writeln!(writer, "tail-line-{}", i);
            let _ = writer.flush();
            // 小间隔让 tail/head 按行处理，更接近真实流式语义
            std::thread::sleep(Duration::from_millis(20));
        }
        // 注：drop writer 关闭写端；tail 读到 EOF 也会自然退出
    }

    // 哨兵在 pipeline 退出后才生效——发送哨兵后 drain 应在 5s 内见到。
    // 关键断言：哨兵能在 timeout 内被接收，即 pipeline 不卡死。
    stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel");
    stdin.flush().expect("flush sentinel");

    let mut acc = String::new();
    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(8));

    // 必须在 timeout 内见到哨兵——若 pipeline 卡死（tail 未被 SIGPIPE 杀掉、
    // shell wait 永远阻塞），则后续 readline 不会触发新 prompt，哨兵不会被打印。
    assert!(
        acc.contains("END_SENTINEL_1"),
        "tail -f | head -n 5 必须在 timeout 内退出（SIGPIPE 路径）；累积输出:\n{}",
        acc
    );

    // pipeline 的 stdout 应出现 5 行 head 输出（前 5 行 tail-line-N）
    // 注：tail -f 在 FIFO 模式下读到的就是写入顺序，前 5 行必然包含 tail-line-1..5
    let saw_line1 = acc.contains("tail-line-1");
    let saw_line5 = acc.contains("tail-line-5");
    assert!(
        saw_line1 && saw_line5,
        "head -n 5 应输出 tail-line-1..5；累积输出:\n{}",
        acc
    );
    // 第 6 行因 head 已退出 + SIGPIPE 杀掉 tail，**不应**出现在 shell stdout 中。
    // 但这是宽松断言——存在「tail 在 SIGPIPE 到达前已 write 第 6 行到 pipe，
    // head 已退出 close 读端但 pipe 缓冲仍持 6 行」的极罕见时序；不强约束。

    drop(stdin);
    drop(guard);
}

#[test]
fn pipeline_command_not_found_does_not_kill_shell() {
    // 错误路径：`cat file | nosuchcmd_xyz`。
    // - find_in_path 对 `nosuchcmd_xyz` 失败 → run_pipeline 写 stderr "command not found"
    //   并 kill 已 spawn 的上游 cat，整条 pipeline 中止；
    // - shell 必须继续 REPL 不退出——下一条 `echo ALIVE` 仍能执行。

    let (mut guard, mut stdin, rx) = spawn_shell();
    let file = make_temp_file(&mut guard, "pipe_notfound", 2);
    let file_str = file.to_str().expect("path utf8");

    stdin
        .write_all(format!("cat {} | nosuchcmd_xyz_abc\n", file_str).as_bytes())
        .expect("write notfound pipe");
    // 紧接一条 echo 验证 shell 活着
    stdin
        .write_all(b"echo ALIVE_AFTER_NOTFOUND\n")
        .expect("write alive echo");
    stdin.flush().expect("flush");

    let mut acc = String::new();
    drain_until(
        &rx,
        &mut acc,
        &["ALIVE_AFTER_NOTFOUND"],
        Duration::from_secs(5),
    );

    assert!(
        acc.contains("ALIVE_AFTER_NOTFOUND"),
        "shell 在 pipeline 中段 not found 后必须继续 REPL；累积输出:\n{}",
        acc
    );

    drop(stdin);
    drop(guard);
}
