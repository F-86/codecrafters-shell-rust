//! `history` 内建渲染逻辑（纯函数）。
//!
//! 文件 IO 路径（`history -r/-w/-a` 与 `$HISTFILE` 启动加载 / 退出保存）
//! 在 `crate::history_io` 模块，与本模块职责正交：本模块拿到 `&[String]`
//! 后只负责渲染，不感知 rustyline `Editor` 或文件系统。

use std::io::{self, Write};

/// `history` 内建渲染：列出会话已执行命令历史，可选限制末尾 N 条。
///
/// 行格式：`"{:>4}  {entry}\n"`（编号右对齐 4 字符宽 + 2 空格 + 命令 + 换行）。
/// 编号从 1 起递增；`history N` 截取末 N 条时编号仍是**全局位置**（不是窗口下标 + 1）。
///
/// 参数语义：
/// - `args` 为空 → 全列出
/// - `args[0]` 解析为 `usize` 成功 → 末 N 条（n=0 → 空输出；n>=len → 全列出）
/// - `args[0]` 解析失败（非数字 / 负数 / 空串）→ err_sink 写
///   `"history: {arg}: numeric argument required\n"` 后早返回 Ok（不阻断 REPL）
///
/// 文件 IO（`-r/-w/-a`）在 `crate::history_io` 模块；本函数只接受 `&[String]` 渲染。
pub fn run_history(
    sink: &mut dyn Write,
    err_sink: &mut dyn Write,
    args: &[String],
    entries: &[String],
) -> io::Result<()> {
    // 1. 解析 args[0] 为 usize；无参数等价 n = len（全列出）
    //    `usize::from_str` 对负数 / 非数字 / 空串均失败 —— 与 bash
    //    「numeric argument required」错误语义天然对齐
    let n = if let Some(arg) = args.first() {
        match arg.parse::<usize>() {
            Ok(v) => v,
            Err(_) => {
                // 错误写 err_sink 失败也不阻断 REPL：吞掉 IO 错误，早返回 Ok
                let _ = writeln!(err_sink, "history: {}: numeric argument required", arg);
                return Ok(());
            }
        }
    } else {
        entries.len()
    };

    // 2. 单一表达式覆盖 n=0 / n>=len / n<len 三种边界：
    //    - n=0      → start = len    → 切片为空 → 0 行输出
    //    - n>=len   → start = 0      → 全列出
    //    - n<len    → start = len-n  → 末尾 n 条
    let start = entries.len().saturating_sub(n);

    // 3. 编号必须用全局下标 `start + i + 1`（不是 `i + 1`）：
    //    bash 语义下 `history N` 显示的编号是条目在完整 history 中的位置。
    for (i, entry) in entries[start..].iter().enumerate() {
        writeln!(sink, "{:>4}  {}", start + i + 1, entry)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Stage「history as a shell builtin」：run_history 格式契约用例 ----

    /// 跑 `run_history` 的薄封装：返回 (stdout, stderr) 字符串对，便于断言。
    fn invoke_history(entries: &[&str]) -> (String, String) {
        let mut sink: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let owned: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
        run_history(&mut sink, &mut err, &[], &owned).expect("run_history");
        (
            String::from_utf8(sink).expect("utf8 stdout"),
            String::from_utf8(err).expect("utf8 stderr"),
        )
    }

    #[test]
    fn history_empty_entries_no_output() {
        // 空 entries：sink / err_sink 均零字节，Ok(())
        let (out, err) = invoke_history(&[]);
        assert!(out.is_empty(), "空 entries 不写 stdout");
        assert!(err.is_empty(), "空 entries 不写 stderr");
    }

    #[test]
    fn history_single_entry_starts_at_one() {
        // 单条：编号 1，4 字符右对齐 + 2 空格 + 命令 + \n
        // 1 字符编号 + 3 个前导空格 = 4 宽
        let (out, err) = invoke_history(&["echo foo"]);
        assert_eq!(out, "   1  echo foo\n");
        assert!(err.is_empty());
    }

    #[test]
    fn history_multiple_entries_increment_from_one() {
        // 多条：编号 1..N 递增，每行独立
        let (out, err) = invoke_history(&["echo foo", "pwd", "history"]);
        let expected = "   1  echo foo\n   2  pwd\n   3  history\n";
        assert_eq!(out, expected);
        assert!(err.is_empty());
    }

    #[test]
    fn history_width_alignment_at_two_digit_boundary() {
        // ≥ 10 条触发 2 位数编号：验证 `{:>4}` 右对齐——1 位编号占 4 宽
        // （3 前导空格 + 1 位数字），2 位编号占 4 宽（2 前导空格 + 2 位数字），
        // 与 bash `history` 输出列对齐一致。
        let entries: Vec<&str> = (1..=12).map(|_| "x").collect();
        let (out, _) = invoke_history(&entries);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 12);
        // 第 1 行：1 位数 → 3 前导空格
        assert_eq!(lines[0], "   1  x");
        // 第 9 行：仍是 1 位数
        assert_eq!(lines[8], "   9  x");
        // 第 10 行：2 位数 → 2 前导空格，命令列位置仍对齐到第 7 列
        assert_eq!(lines[9], "  10  x");
        assert_eq!(lines[11], "  12  x");
        // 显式断言：编号字段 + 2 空格分隔 + 命令的列对齐
        for line in &lines {
            // 前 4 字符是编号区，第 5/6 字符必须是 "  "，第 7 字符开始是命令
            assert_eq!(&line[4..6], "  ", "编号后必须紧跟 2 空格分隔");
            assert_eq!(&line[6..], "x", "命令字段从第 7 字符开始");
        }
    }

    // ---- Stage「history N」：args 参数化的语义契约用例 ----

    /// 带 args 版的 `run_history` 薄封装：返回 (stdout, stderr) 便于断言。
    /// 与 `invoke_history` 并存而非合并：保留无参数路径的单测可读性。
    fn invoke_history_with_args(entries: &[&str], args: &[&str]) -> (String, String) {
        let mut sink: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let owned_entries: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
        let owned_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        run_history(&mut sink, &mut err, &owned_args, &owned_entries).expect("run_history");
        (
            String::from_utf8(sink).expect("utf8 stdout"),
            String::from_utf8(err).expect("utf8 stderr"),
        )
    }

    #[test]
    fn history_with_n_smaller_than_len_uses_global_numbering() {
        // 4 条 history + `history 2` → 输出末 2 条但编号是 3/4（全局位置）
        // 这是本阶段最易写错的点：必须用 start+i+1 而非 i+1
        let (out, err) = invoke_history_with_args(&["a", "b", "c", "d"], &["2"]);
        assert_eq!(out, "   3  c\n   4  d\n");
        assert!(err.is_empty());
    }

    #[test]
    fn history_with_n_zero_no_output() {
        // n=0：start=len，切片为空，0 字节输出
        let (out, err) = invoke_history_with_args(&["a", "b"], &["0"]);
        assert!(out.is_empty(), "n=0 不写 stdout");
        assert!(err.is_empty());
    }

    #[test]
    fn history_with_n_greater_than_len_full_list() {
        // n>=len：saturating_sub 返回 0，等价无参数全列
        let (out, err) = invoke_history_with_args(&["a", "b"], &["99"]);
        assert_eq!(out, "   1  a\n   2  b\n");
        assert!(err.is_empty());
    }

    #[test]
    fn history_with_n_equal_to_len_full_list() {
        // n=len：start=0，全列出，编号 1..=len
        let (out, err) = invoke_history_with_args(&["a", "b", "c"], &["3"]);
        assert_eq!(out, "   1  a\n   2  b\n   3  c\n");
        assert!(err.is_empty());
    }

    #[test]
    fn history_non_numeric_arg_writes_stderr() {
        // 非数字参数 → err_sink 写错误，sink 空，函数仍 Ok（REPL 不阻断）
        let (out, err) = invoke_history_with_args(&["a"], &["abc"]);
        assert!(out.is_empty(), "非法参数不写 stdout");
        assert_eq!(err, "history: abc: numeric argument required\n");
    }

    #[test]
    fn history_negative_arg_writes_stderr() {
        // 负数 "-5" 走 usize::from_str 解析失败 → 同样 numeric required 错误
        let (out, err) = invoke_history_with_args(&["a"], &["-5"]);
        assert!(out.is_empty());
        assert_eq!(err, "history: -5: numeric argument required\n");
    }

    #[test]
    fn history_extra_args_uses_first_only() {
        // 多余参数静默忽略（对齐 bash）：`history 2 ignored junk` 等价 `history 2`
        let (out, err) =
            invoke_history_with_args(&["a", "b", "c"], &["2", "ignored", "junk"]);
        assert_eq!(out, "   2  b\n   3  c\n");
        assert!(err.is_empty());
    }
}
