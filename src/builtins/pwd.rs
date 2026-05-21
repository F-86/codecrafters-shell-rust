//! `pwd` 内建：打印当前工作目录。

use std::io::{self, Write};

/// `pwd` 内建：打印当前工作目录的绝对路径。
/// `current_dir()` 内部调用 `getcwd(2)`，由 OS 保证返回绝对路径。
/// 目录被删除 / 无权限等异常场景：错误信息写入 err_sink，可被 `2>` 重定向到文件。
pub fn run_pwd(sink: &mut dyn Write, err_sink: &mut dyn Write) -> io::Result<()> {
    match std::env::current_dir() {
        Ok(path) => writeln!(sink, "{}", path.display()),
        Err(e) => {
            // 写入 err_sink 的失败本身仍 fallback 到顶层 eprintln!，避免双重错误丢失
            writeln!(err_sink, "pwd: {}", e)
        }
    }
}
