//! `cd` 内建：切换当前工作目录。

use std::io::Write;

/// `cd` 内建：切换当前工作目录。
///
/// 取首个参数作为目标路径；支持绝对路径、相对路径（./、../、子目录名）与 `~`
/// 相对路径由内核基于当前进程 cwd 解析，无需在此做字符串展开。
/// 无参数场景本阶段不覆盖，静默跳过。
/// 注：cd 不产 stdout 输出，故不接 sink；错误信息走 err_sink，可被 `2>` 捕获。
///
/// `~` 展开：本阶段仅匹配精确 `~`，不处理 `~/subdir` 或 `~user` 形式；
/// HOME 缺失时按统一错误格式输出，避免 unwrap 导致 REPL 中断。
///
/// 直接调用 `chdir(2)`：失败时 OS 保证 cwd 不变，无 TOCTOU 风险。
/// 不存在 / 非目录 / 无权限等失败统一打印同一错误信息以匹配测试期望。
/// 错误信息回显用户原始输入 target，不展开为 home 路径（与 bash 行为一致）。
pub fn run_cd(err_sink: &mut dyn Write, args: &[String]) {
    if let Some(target) = args.first() {
        let resolved = if target == "~" {
            match std::env::var("HOME") {
                Ok(home) => home,
                Err(_) => {
                    let _ = writeln!(err_sink, "cd: {}: No such file or directory", target);
                    return;
                }
            }
        } else {
            target.clone()
        };
        if std::env::set_current_dir(&resolved).is_err() {
            let _ = writeln!(err_sink, "cd: {}: No such file or directory", target);
        }
    }
}
