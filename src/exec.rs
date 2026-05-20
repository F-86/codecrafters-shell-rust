//! 外部命令分支：在 PATH 命中后用 `Command` spawn 子进程，按 `ParsedCommand` 物化
//! stdout / stderr 的 Stdio（重定向文件或 inherit 终端），失败时降级回错误提示。
//!
//! 把这段从 REPL 主循环抽出后，`main.rs` 不再需要 `std::process::Command` / `Stdio`
//! / `CommandExt` 等 use；外部命令未命中 PATH 时仍通过传入的 `err_sink` 写
//! "command not found"（可被 `2>` 捕获），保持与拆分前完全一致的语义。

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use crate::builtins::find_in_path;
use crate::parser::ParsedCommand;
use crate::redirect::open_file_for_redirect;

/// 执行非内建命令。
///
/// 参数：
/// - `cmd`：用户输入的命令名（用作 `argv[0]`，与 bash 行为一致——不替换为绝对路径）；
/// - `line`：用户输入的整行原文，仅在 not-found / spawn 失败时拼接 "command not found" 错误使用；
/// - `args`：除命令名外的参数切片；
/// - `parsed`：完整解析结果，提供 stdout/stderr 重定向元信息；
/// - `sink` / `err_sink`：本轮已打开的写句柄。**所有权由调用方移交给本函数**——
///   在 spawn 子进程前 `drop` 掉，避免与子进程对同一文件 fd 产生竞态。
///
/// 关键路径注释：
/// - sink 走的是 `Box<dyn Write>` 抽象，运行时无法把它「拆回」具体类型，
///   故对外部命令重新从 `parsed.{stdout,stderr}_redirect` 物化一次 stdio。
///   共享 `open_file_for_redirect` helper 保证此处与 sink 打开模式一致：
///   append 标志为 true 时复用 `OpenOptions.append(true)`，文件原有内容保留；
///   为 false 时复用 `File::create`（截断）。
/// - err_sink 已在上游 `open_err_sink` 阶段打开过同一文件；这里再次打开属同一模式
///   （append / truncate），对 truncate 模式而言是再次 truncate 为空（语义不变），
///   对 append 模式而言只是另开一个 fd 共享 O_APPEND 语义（内核保证两 fd 写入都
///   原子追加到末尾，不会互相覆盖）。
/// - spawn / wait 失败时降级，避免 REPL 中断；此时 err_sink 已被 drop，
///   回退到父进程 stderr（仍可见但不被 `2>` 捕获——属罕见极端情况，可接受的退化语义）。
pub fn run_external(
    cmd: &str,
    line: &str,
    args: &[String],
    parsed: &ParsedCommand,
    sink: Box<dyn Write>,
    mut err_sink: Box<dyn Write>,
) {
    let Some(path) = find_in_path(cmd) else {
        // 未在 PATH 命中：command not found 走 err_sink（可被 `2>` 捕获）
        let _ = writeln!(&mut *err_sink, "{}: command not found", line);
        return;
    };

    // 在 spawn 子进程之前**释放** sink / err_sink 持有的文件 fd 拥有权——
    // 重定向时把 File 直接转移给 Command::stdout / .stderr，由子进程 inherit；
    // 无重定向时让父进程继承自己的 stdout / stderr（默认即 inherit）。
    let stdio = match parsed.stdout_redirect.as_deref() {
        Some(target) => match open_file_for_redirect(target, parsed.stdout_append) {
            Ok(f) => Stdio::from(f),
            Err(e) => {
                eprintln!("{}: {}: {}", cmd, target, e);
                return;
            }
        },
        None => Stdio::inherit(),
    };
    let err_stdio = match parsed.stderr_redirect.as_deref() {
        Some(target) => match open_file_for_redirect(target, parsed.stderr_append) {
            Ok(f) => Stdio::from(f),
            Err(e) => {
                eprintln!("{}: {}: {}", cmd, target, e);
                return;
            }
        },
        None => Stdio::inherit(),
    };
    // 提前丢弃父进程内 sink / err_sink 对同一文件的写句柄，避免与子进程
    // 的截断/写入产生竞态（虽然内核 fd 独立，但语义上更干净）。
    drop(sink);
    drop(err_sink);

    // argv[0] 必须是用户输入的命令名而非完整路径（与 bash 行为一致）
    let status = Command::new(&path)
        .arg0(cmd)
        .args(args)
        .stdout(stdio)
        .stderr(err_stdio)
        .status();
    if status.is_err() {
        eprintln!("{}: command not found", line);
    }
}
