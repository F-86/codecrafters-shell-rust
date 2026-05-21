//! 命令名 TAB 状态机：builtin + PATH executables 候选源、单/多候选 LCP 扩展、双 TAB 列出。
//!
//! 行为概述：
//! - 候选源：`BUILTINS` 常量（builtin 优先）+ `path_executables`（启动期缓存）
//! - 触发条件：`line[..pos]` 不含空白
//! - 去重：同名时 builtin 优先，PATH 同名忽略
//! - 多候选：先尝试 LCP 扩展；不可扩展时进入「BEL → 二次 TAB 列出 + 重画」双态机
//!
//! 详见 docs/DESIGN_DECISIONS.md#completion-state-machine。

use std::collections::HashSet;
use std::io::{self, Write};

use rustyline::completion::Pair;
use rustyline::Result;

use super::helpers::longest_common_prefix;
use super::ShellHelper;
use crate::builtins::BUILTINS;

/// 命令名 TAB 补全：在首词位置按前缀匹配 builtin 与 PATH 中可执行文件。
///
/// 入口契约：调用方已确认 `prefix == line[..pos]` 不含空白；命令名分支被触发
/// 时对侧两套 Cell（`last_tab_arg_key` / `last_tab_script_key`）已由 `mod.rs`
/// dispatch 清空。
pub(super) fn complete_command(
    helper: &ShellHelper,
    prefix: &str,
    pos: usize,
) -> Result<(usize, Vec<Pair>)> {
    // 阶段 1：收集去重后的候选名（builtin 优先，PATH 内部及与 builtin 同名均跳过）
    let mut names: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for name in BUILTINS {
        if name.starts_with(prefix) {
            seen.insert(*name);
            names.push((*name).to_string());
        }
    }
    for name in &helper.path_executables {
        if name.starts_with(prefix) && !seen.contains(name.as_str()) {
            seen.insert(name.as_str());
            names.push(name.clone());
        }
    }

    // 阶段 2：按候选数三态分支
    match names.len() {
        0 => {
            helper.last_tab_prefix.set(None);
            Ok((pos, Vec::new()))
        }
        1 => {
            helper.last_tab_prefix.set(None);
            let name = names.into_iter().next().unwrap();
            let pair = Pair {
                display: name.clone(),
                replacement: format!("{} ", name),
            };
            Ok((0, vec![pair]))
        }
        _ => {
            // 多候选：先字母序排序，再判断 LCP 是否可扩展
            names.sort();
            let last = names.last().unwrap();
            let lcp = longest_common_prefix(&names[0], last);
            if lcp.len() > prefix.len() {
                // LCP 扩展：清状态 + 让 rustyline 把 line[0..pos] 替换为 lcp（不带尾空格）
                helper.last_tab_prefix.set(None);
                let s = lcp.to_string();
                let pair = Pair {
                    display: s.clone(),
                    replacement: s,
                };
                return Ok((0, vec![pair]));
            }
            // LCP == prefix：无法扩展，进入双 TAB 状态机
            let prev = helper.last_tab_prefix.take();
            let same_as_prev = prev.as_deref() == Some(prefix);
            if same_as_prev {
                // 二次 TAB：列出 + 重画提示符；状态机已清空（take 取走）
                let joined = names.join("  ");
                print!("\n{}\n$ {}", joined, prefix);
                let _ = io::stdout().flush();
            } else {
                // 首次 TAB（或前缀变化的新一轮）：响铃并记忆当前前缀
                print!("\x07");
                let _ = io::stdout().flush();
                helper.last_tab_prefix.set(Some(prefix.to_string()));
            }
            Ok((pos, Vec::new()))
        }
    }
}
