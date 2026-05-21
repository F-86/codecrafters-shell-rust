//! 单命令外部进程：PATH 解析、stdio 物化、前台 / 后台分支。
//!
//! 前台分支：`command.status()` 同步等待。
//! 后台分支：`command.spawn()` 不 wait + 打印 `[N] PID` 通知 + 入 jobs_table。
//! 通知行走父进程 stdout（不复用 sink）——与 bash 一致：job 控制信息不被用户
//! `>` 重定向捕获到文件。
//!
//! 后台子进程的 stdio 继承走 [`Stdio::inherit()`] + dup2，fd 复制独立存活，
//! 与 Child 是否被 wait/drop 完全无关，是 POSIX fd 继承的标准语义。
//!
//! 详见 [docs/DESIGN_DECISIONS.md#background-reaping](../../docs/DESIGN_DECISIONS.md#background-reaping)。

use std::cell::RefCell;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::rc::Rc;

use crate::builtins::{allocate_job_id, find_in_path, Job, JobStatus};
use crate::parser::ParsedCommand;
use crate::redirect::open_file_for_redirect;

/// 执行非内建命令。
///
/// 参数：
/// - `cmd` / `args`：用户输入命令名（用作 `argv[0]`）+ 参数切片
/// - `line`：完整输入原文，仅 not-found / spawn 失败时拼 "command not found"
/// - `parsed`：stdout/stderr 重定向元信息 + `background` 标志
/// - `sink` / `err_sink`：本轮已打开句柄；spawn 前 drop，避免与子进程 fd 竞态
/// - `jobs_table`：仅后台分支 spawn 成功后写入；前台与失败路径不触碰
///
/// 后台作业编号由 [`allocate_job_id`] 基于 `jobs_table` 当前内容计算「最小可用」
/// 正整数。详见 [docs/DESIGN_DECISIONS.md#background-reaping](../../docs/DESIGN_DECISIONS.md#background-reaping)。
pub fn run_external(
    cmd: &str,
    line: &str,
    args: &[String],
    parsed: &ParsedCommand,
    sink: Box<dyn Write>,
    mut err_sink: Box<dyn Write>,
    jobs_table: &Rc<RefCell<Vec<Job>>>,
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
    let mut command = Command::new(&path);
    command
        .arg0(cmd)
        .args(args)
        .stdout(stdio)
        .stderr(err_stdio);

    if parsed.background {
        // 后台分支：spawn 不 wait，立即打印 `[N] PID` 通知后返回。
        // 通知行走父进程 stdout（不复用已 drop 的 sink）——与 bash 一致：
        // job 控制信息不被用户 `>` 重定向捕获到文件。
        match command.spawn() {
            Ok(child) => {
                let pid = child.id();
                // 借用即用即释放：`jobs_table.borrow()` 是临时表达式，语句末 drop，
                // 确保下方 `borrow_mut()` 不与之重叠（无 RefCell 双借 panic）。
                let id = allocate_job_id(&jobs_table.borrow());
                let _ = writeln!(std::io::stdout(), "[{}] {}", id, pid);
                jobs_table.borrow_mut().push(Job {
                    id,
                    pid,
                    command: parsed.argv.join(" "),
                    status: JobStatus::Running,
                    children: vec![child],
                });
            }
            Err(_) => {
                eprintln!("{}: command not found", line);
            }
        }
        return;
    }

    // 前台分支：保持既有 `.status()` 同步等待语义不变。
    let status = command.status();
    if status.is_err() {
        eprintln!("{}: command not found", line);
    }
}
