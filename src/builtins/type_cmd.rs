//! `type` 内建：查询目标名是 builtin、PATH 中可执行文件还是 not found。
//!
//! 模块文件名为 `type_cmd.rs` 而非 `type.rs` 以规避 Rust 关键字 `type`。

use std::io::{self, Write};

use super::path::find_in_path;
use super::BUILTINS;

/// `type` 内建：查询目标名是 builtin、PATH 中可执行文件还是 not found。
/// `builtin` / PATH 命中走 stdout sink；`not found` 走 err_sink（与 bash 行为一致），
/// 可被 `2>` 重定向到文件。无参数时静默（与既有行为一致）。
pub fn run_type(
    sink: &mut dyn Write,
    err_sink: &mut dyn Write,
    args: &[String],
) -> io::Result<()> {
    let Some(target) = args.first() else {
        return Ok(());
    };
    let target = target.as_str();
    if BUILTINS.contains(&target) {
        writeln!(sink, "{} is a shell builtin", target)
    } else if let Some(path) = find_in_path(target) {
        writeln!(sink, "{} is {}", target, path.display())
    } else {
        writeln!(err_sink, "{}: not found", target)
    }
}
