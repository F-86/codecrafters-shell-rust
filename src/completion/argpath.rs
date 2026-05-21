//! 参数位置 TAB 状态机：路径补全（cwd / 嵌套目录）+ 双 TAB 节奏。
//!
//! 状态 key：`(dir_part, name_prefix)` 二元组，跨命令名节奏复用——
//! 用户在两次 TAB 之间改了命令名但 token 切分结果相同时仍算同一轮。
//!
//! 详见 docs/DESIGN_DECISIONS.md#completion-state-machine。

use std::io::{self, Write};
use std::path::Path;

use rustyline::completion::Pair;
use rustyline::Result;

use super::helpers::{
    classify_path, extract_arg_prefix, format_arg_completion, longest_common_prefix,
    match_files_in_dir, split_dir_and_name, MatchKind,
};
use super::ShellHelper;

/// 参数位置文件名 / 目录补全。
///
/// 入口契约：调用方已确认 `line[..pos]` 至少含一个空白（已离开命令名区）。
///
/// 行为：
/// - prefix 提取失败（tokenize 错误等）→ 静默 no-op，不响铃。
/// - 末尾空白 → `prefix = ""`：等价"列出 cwd / dir 全部 entry"。
/// - 候选数 0 → BEL + 清状态。
/// - 候选数 = 1 → 替换 `[pos - prefix.len(), pos)` 为 `<full> ` 或 `<full>/`。
/// - 候选数 ≥ 2：先尝试 LCP 扩展（>name_prefix 时），否则进入双 TAB 状态机。
pub(super) fn complete_filename_arg(
    helper: &ShellHelper,
    line: &str,
    pos: usize,
) -> Result<(usize, Vec<Pair>)> {
    // 文件名分支被触发：脚本分支双 TAB 节奏作废（命令名分支由统一出口处清）。
    helper.last_tab_script_key.set(None);

    let line_to_pos = &line[..pos];

    // 1. 提取前缀；tokenize 失败 → 静默 no-op。
    let prefix = match extract_arg_prefix(line_to_pos) {
        Some(p) => p,
        None => {
            helper.last_tab_arg_key.set(None);
            helper.last_tab_prefix.set(None);
            return Ok((pos, Vec::new()));
        }
    };

    // 2. 字面对齐校验：本 stage 测试不含引号/转义，prefix 应与 line 末尾字面一致；
    //    不一致说明 tokenize 做了剥离，按 no-op 退避。
    if prefix.len() > pos {
        helper.last_tab_arg_key.set(None);
        helper.last_tab_prefix.set(None);
        return Ok((pos, Vec::new()));
    }
    let start = pos - prefix.len();
    if &line[start..pos] != prefix.as_str() {
        helper.last_tab_arg_key.set(None);
        helper.last_tab_prefix.set(None);
        return Ok((pos, Vec::new()));
    }

    // 3. 按最后一个 '/' 切分目录与叶子前缀。
    let (dir_part, name_prefix) = split_dir_and_name(&prefix);
    let scan_dir: &Path = if dir_part.is_empty() {
        Path::new(".")
    } else {
        Path::new(dir_part)
    };
    let mut candidates = match_files_in_dir(scan_dir, name_prefix);

    // 命令名分支状态独立但语义上互斥：任一分支被触发都视为「另一边的节奏已断」
    helper.last_tab_prefix.set(None);

    // 4. 候选数分支
    match candidates.len() {
        0 => {
            helper.last_tab_arg_key.set(None);
            print!("\x07");
            let _ = io::stdout().flush();
            Ok((pos, Vec::new()))
        }
        1 => {
            helper.last_tab_arg_key.set(None);
            let entry = candidates.into_iter().next().unwrap();
            let full = format!("{}{}", dir_part, entry);
            let kind = classify_path(Path::new(&full));
            Ok((start, vec![format_arg_completion(&full, kind)]))
        }
        _ => {
            candidates.sort();

            // 4a. LCP 扩展：候选叶子名 LCP 长于 name_prefix → 替换 [start, pos) 为
            //     `dir_part + lcp`（不带尾空格 / `/`），让用户继续打字以收敛候选。
            let lcp = longest_common_prefix(&candidates[0], candidates.last().unwrap());
            if lcp.len() > name_prefix.len() {
                helper.last_tab_arg_key.set(None);
                let replacement = format!("{}{}", dir_part, lcp);
                let pair = Pair {
                    display: replacement.clone(),
                    replacement,
                };
                return Ok((start, vec![pair]));
            }

            // 4b. LCP 不可扩展：进入双 TAB 状态机。
            let current_key = (dir_part.to_string(), name_prefix.to_string());
            let prev = helper.last_tab_arg_key.take();
            let same_as_prev = prev.as_ref() == Some(&current_key);
            if same_as_prev {
                // 二次 TAB：列出 + 重画提示符。
                // 每候选 stat 一次以判类型（目录拼尾 '/'）；候选数典型 ≤ 3，开销可忽略。
                let listed: Vec<String> = candidates
                    .iter()
                    .map(|name| {
                        let full = format!("{}{}", dir_part, name);
                        match classify_path(Path::new(&full)) {
                            MatchKind::Directory => format!("{}/", name),
                            MatchKind::File => name.clone(),
                        }
                    })
                    .collect();
                let joined = listed.join("  ");
                // 重画整段 line[..pos]（含命令名 + 已输入的参数部分），不能只重画 prefix。
                print!("\n{}\n$ {}", joined, line_to_pos);
                let _ = io::stdout().flush();
            } else {
                // 首次 TAB（或 key 变化的新一轮）：BEL + 记忆当前 key
                print!("\x07");
                let _ = io::stdout().flush();
                helper.last_tab_arg_key.set(Some(current_key));
            }
            Ok((pos, Vec::new()))
        }
    }
}
