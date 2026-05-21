//! 集成测试：pipeline 中 builtin 段端到端验证（前瞻能力）。
//!
//! 覆盖：
//! - 首段 builtin：`echo hello | wc -c` 应输出 6（"hello\n" = 6 字节）；
//! - 末段 builtin：`cat file | echo overridden` 应输出 "overridden"
//!   （验证 builtin 不读 stdin，上游 cat 被允许独立运行 / 输出被丢弃）；
//! - 中段 builtin：`echo a | echo b | wc -c` 输出 2（"b\n"）——验证中段 builtin
//!   缓冲方案 + 上游被忽略；
//! - 不支持的 builtin 在 pipeline 中按 `command not found` 处理（如 `cd` / `jobs` /
//!   `complete` / `exit`），避免污染父 shell 状态。
//!
//! 设计要点与 `pipeline_basic.rs` 一致：spawn 子进程 + 哨兵切窗口 + drain_until 5s 超时。

#![cfg(unix)]

mod common;

use std::io::Write;
use std::time::Duration;

use common::{drain_until, spawn_shell, unique_tmp_path};

#[test]
fn builtin_echo_first_stage_pipes_into_wc() {
    // `echo hello | wc -c` → "hello\n" 共 6 字节
    let (guard, mut stdin, rx) = spawn_shell();

    stdin
        .write_all(b"echo hello | wc -c\n")
        .expect("write pipe");
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
    let found = window.lines().any(|l| l.trim() == "6");
    assert!(
        found,
        "`echo hello | wc -c` 必须输出 6（含换行）；窗口:\n{}",
        window
    );

    drop(stdin);
    drop(guard);
}

#[test]
fn builtin_echo_last_stage_ignores_upstream_stdin() {
    // `printf '...' | echo overridden`：builtin echo 不读 stdin，
    // 上游 printf 的输出被 Stdio::null() 等价丢弃，echo 直接输出 "overridden\n"。
    //
    // 用 printf 而非 cat <file>：避免 fifo / 临时文件依赖，简化测试。
    let (guard, mut stdin, rx) = spawn_shell();

    // 注：上游用 `echo upstream` 即可——其 stdout 走 pipe 但末段 builtin echo
    // 不读，pipe 写端在末段 spawn 时无对应读端句柄（buffer in parent 路径不接管），
    // 触发 SIGPIPE 让上游 echo 自然退出（echo 写完一行就退出，无 SIGPIPE 路径）。
    stdin
        .write_all(b"echo upstream_data | echo overridden\n")
        .expect("write pipe");
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
    // 必须含 "overridden"，且**不应**含 "upstream_data"
    // （上游输出被本实现的 Buffer/Null 路径丢弃）
    assert!(
        window.contains("overridden"),
        "末段 builtin echo 必须输出 `overridden`；窗口:\n{}",
        window
    );
    // 注：宽松检查——上游输出可能在 buffer 内但不会写到终端，
    // 故 window 不应含 upstream_data。
    assert!(
        !window.contains("upstream_data"),
        "末段 builtin 不读 stdin，上游输出不得泄漏到终端；窗口:\n{}",
        window
    );

    drop(stdin);
    drop(guard);
}

#[test]
fn builtin_middle_stage_buffer_passes_through() {
    // 三段：`echo a | echo b | wc -c`
    // - 首段 echo a：缓冲到 Vec<u8> "a\n"
    // - 中段 echo b：上游被丢弃，自己输出 "b\n" 缓冲到下一段 stdin
    // - 末段 wc -c：从 stdin 读 "b\n" 输出 2
    //
    // 关键验证：中段 builtin 的缓冲路径正确——上游 echo a 输出被丢弃、
    // 中段 echo b 输出正确喂入末段 wc -c stdin。
    let (guard, mut stdin, rx) = spawn_shell();

    stdin
        .write_all(b"echo a | echo b | wc -c\n")
        .expect("write three-stage builtin pipe");
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
    let found = window.lines().any(|l| l.trim() == "2");
    assert!(
        found,
        "三段 builtin pipeline 应输出 2（`b\\n` 2 字节）；窗口:\n{}",
        window
    );

    drop(stdin);
    drop(guard);
}

#[test]
fn pwd_first_stage_buffer_feeds_grep() {
    // `pwd | wc -c`：pwd 输出当前 cwd + 换行，wc -c 数字节数 > 1。
    // 主要验证 pwd builtin 在首段缓冲路径下能产出非空数据并被下游消费。
    let (guard, mut stdin, rx) = spawn_shell();

    stdin.write_all(b"pwd | wc -c\n").expect("write pwd | wc");
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
    // 取第一个数字行作为 wc -c 输出；应该 > 1（cwd 至少 "/" + 换行 = 2 字节）
    let count: u32 = window
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .next()
        .unwrap_or(0);
    assert!(
        count > 1,
        "`pwd | wc -c` 应产出 > 1 的字节计数；窗口:\n{}",
        window
    );

    drop(stdin);
    drop(guard);
}

#[test]
fn unsupported_builtin_in_pipeline_falls_to_not_found() {
    // pipeline 中 `cd` / `jobs` / `exit` / `complete` 不在本实现的 builtin-in-pipeline
    // 支持范围内——按 PATH 查找走 not found 路径，shell 不退出、不改 cwd。
    //
    // 用 `cd` 验证：若 pipeline 段意外执行了 cd 副作用会改 shell cwd；
    // 测试断言之后 `pwd` 仍返回原 cwd 即可。
    let (guard, mut stdin, rx) = spawn_shell();

    // 先记录 cwd
    stdin.write_all(b"pwd\n").expect("write pwd1");
    stdin
        .write_all(b"echo END_SENTINEL_1\n")
        .expect("write sentinel 1");
    stdin.flush().expect("flush 1");

    let mut acc = String::new();
    drain_until(&rx, &mut acc, &["END_SENTINEL_1"], Duration::from_secs(5));
    let pwd_before = {
        let window = match acc.find("END_SENTINEL_1") {
            Some(idx) => &acc[..idx],
            None => acc.as_str(),
        };
        window
            .lines()
            .find(|l| l.starts_with('/'))
            .map(|s| s.to_string())
            .expect("locate pwd1 output")
    };

    // pipeline 中调用 cd /tmp（不支持的 builtin in pipeline）：
    // 走 external 路径 find_in_path("cd") → None → 报 not found；
    // shell 的 cwd 应保持不变。
    let target = unique_tmp_path("ignored_target_dir");
    let _ = std::fs::create_dir_all(&target);
    // 故意不 push 到 cleanup（让 OS 清/或 RAII 不持有路径），简化逻辑
    stdin
        .write_all(format!("echo go | cd {}\n", target.display()).as_bytes())
        .expect("write cd-in-pipe");
    stdin
        .write_all(b"pwd\n")
        .expect("write pwd2");
    stdin
        .write_all(b"echo END_SENTINEL_2\n")
        .expect("write sentinel 2");
    stdin.flush().expect("flush 2");

    drain_until(&rx, &mut acc, &["END_SENTINEL_2"], Duration::from_secs(5));
    let region2 = {
        let s = acc.find("END_SENTINEL_1").map(|i| i + "END_SENTINEL_1".len()).unwrap_or(0);
        let e = acc.find("END_SENTINEL_2").unwrap_or(acc.len());
        acc[s..e].to_string()
    };
    let pwd_after = region2
        .lines()
        .find(|l| l.starts_with('/'))
        .map(|s| s.to_string())
        .expect("locate pwd2 output");

    assert_eq!(
        pwd_before, pwd_after,
        "pipeline 中 cd 必须不改变父 shell cwd（按 not found 处理）\nbefore: {}\nafter: {}",
        pwd_before, pwd_after
    );

    // 清理 target 目录
    let _ = std::fs::remove_dir_all(&target);

    drop(stdin);
    drop(guard);
}
