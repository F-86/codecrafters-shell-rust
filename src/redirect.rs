//! 重定向 sink 打开 helpers：把 `stdout_redirect` / `stderr_redirect` 与 `append` 标志
//! 转换为可写的 `Box<dyn Write>`，或者把目标 path 物化为子进程可用的 [`std::fs::File`]。
//!
//! 三个函数共享同一套打开语义，避免「截断 / 追加」逻辑在 builtin sink 与外部命令 stdio
//! 两个站点分头实现导致语义漂移。

use std::fs::{File, OpenOptions};
use std::io::{self, Write};

/// 按重定向语义打开目标文件并返回 [`File`]，供 sink 与外部命令 [`std::process::Stdio::from`] 共享：
/// - `append == true` → `OpenOptions::new().create(true).append(true).open(path)`
///   （文件不存在则创建；存在则保留原有内容，新写入从末尾追加）。
/// - `append == false` → `File::create(path)` 等价于 O_WRONLY|O_CREAT|O_TRUNC，
///   即既有截断重定向行为。
///
/// 抽出此 helper 后，`open_sink` / `open_err_sink` 与外部命令分支共享同一打开语义，
/// 避免「截断 / 追加」逻辑在多个站点重复（防止后续遗漏其中一处导致语义不一致）。
pub fn open_file_for_redirect(path: &str, append: bool) -> io::Result<File> {
    if append {
        OpenOptions::new().create(true).append(true).open(path)
    } else {
        File::create(path)
    }
}

/// 根据 `stdout_redirect` 与 `append` 标志准备 sink：
/// - `None` → 锁定的 stdout 句柄；
/// - `Some(path)` + `append=false` → `File::create(path)`（不存在则创建、存在则截断覆盖）；
/// - `Some(path)` + `append=true`  → `OpenOptions.append(true).create(true).open(path)`。
///
/// 返回 `Box<dyn Write>` 让调用方对两种来源无感；打开失败则把错误反馈给调用方，
/// 由 REPL 主循环统一打印到 stderr 并跳过本轮命令执行（保证 REPL 不中断）。
pub fn open_sink(stdout_redirect: Option<&str>, append: bool) -> io::Result<Box<dyn Write>> {
    match stdout_redirect {
        Some(path) => Ok(Box::new(open_file_for_redirect(path, append)?)),
        // 注：返回 `io::Stdout` 而非 `StdoutLock<'_>` 以规避借用生命周期约束。
        // 多线程场景下 `Stdout` 内部已有锁，对 REPL 单线程使用场景表现等价。
        None => Ok(Box::new(io::stdout())),
    }
}

/// 根据 `stderr_redirect` 与 `append` 标志准备 err_sink，与 [`open_sink`] 完全对称：
/// - `None` → `io::stderr()` 句柄（终端可见，bash 默认行为）；
/// - `Some(path)` + `append=false` → `File::create(path)`（截断/创建，即使无 stderr 写入
///   文件也会被预先创建为空，与 bash 一致）；
/// - `Some(path)` + `append=true`  → `OpenOptions.append(true).create(true).open(path)`。
pub fn open_err_sink(stderr_redirect: Option<&str>, append: bool) -> io::Result<Box<dyn Write>> {
    match stderr_redirect {
        Some(path) => Ok(Box::new(open_file_for_redirect(path, append)?)),
        None => Ok(Box::new(io::stderr())),
    }
}
