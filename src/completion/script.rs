//! 命令级补全脚本分支：`complete -C` 注册的命令通过子进程驱动 TAB 候选。
//!
//! 包含三件套：
//! - `CompleterContext` + `extract_completer_context`：从 `line[..pos]` 提取
//!   argv (cmd / current_word / prev_word) + literal_len 用于替换起点。
//! - `run_completer_script`：spawn 子进程并捕获 stdout，处理 env (COMP_LINE / COMP_POINT)。
//! - `parse_completer_stdout`：把脚本 stdout 切分为候选列表（容忍 CRLF / 空行）。
//! - `complete_with_script`：组合上面三件套 + 双 TAB 状态机驱动，对接 `Completer::complete`。
//!
//! 详见 docs/DESIGN_DECISIONS.md#completion-state-machine。

use std::collections::HashMap;
use std::io::{self, Write};
use std::process::Command;

use rustyline::completion::Pair;
use rustyline::Result;

use super::helpers::longest_common_prefix;
use super::ShellHelper;
use crate::parser::tokenize;

/// 命令级补全上下文：`Completer::complete` dispatch 阶段从 `line[..pos]` 提取的
/// 全部脚本调用所需信息。字段语义与 bash COMP_* 对齐。
///
/// - `cmd`：`argv[1]`，命令名（tokenize 后的首 token，已剥引号）。
/// - `current_word`：`argv[2]`，当前正被补全的词（tokenize 后值）。末尾空白时为空串。
/// - `prev_word`：`argv[3]`，前一词。**与 bash `complete -C` 语义对齐**：
///   prev 在完整 tokens 区间（含 cmd）中查找——`git rem<TAB>` 的 prev=`"git"`。
/// - `literal_len`：光标处『当前词原始字面段』的字节长度，用于计算 replacement 起点。
#[derive(Debug)]
pub(super) struct CompleterContext {
    pub(super) cmd: String,
    pub(super) current_word: String,
    pub(super) prev_word: String,
    pub(super) literal_len: usize,
}

/// 从光标左侧子串提取命令级补全上下文。
///
/// 命中条件：
/// - `line_to_pos` 含至少一个空白（已离开命令名区）
/// - tokenize 成功（未闭合引号 / 行尾孤立反斜杠 → None，外层回退到文件名补全）
/// - tokenize 后至少有一个 token（cmd 存在）
///
/// prev_word 规则（含 cmd，与 bash `complete -C` 语义一致）：
/// - 末尾空白：current = ""，prev = tokens 最后一项（仅 cmd 时 prev = cmd）
/// - 末尾非空白：current = tokens 最后一项；prev = tokens 倒数第二项（len<2 时返 ""）
pub(super) fn extract_completer_context(line_to_pos: &str) -> Option<CompleterContext> {
    if !line_to_pos.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    let tokens = tokenize(line_to_pos, &HashMap::new()).ok()?;
    if tokens.is_empty() {
        return None;
    }
    let cmd = tokens[0].clone();

    let trailing_ws = line_to_pos
        .chars()
        .next_back()
        .map_or(false, |c| c.is_whitespace());

    let (current_word, prev_word) = if trailing_ws {
        let prev = tokens.last().cloned().unwrap_or_default();
        (String::new(), prev)
    } else {
        let current = tokens.last().cloned().unwrap_or_default();
        let prev = if tokens.len() >= 2 {
            tokens[tokens.len() - 2].clone()
        } else {
            String::new()
        };
        (current, prev)
    };

    let literal_len = if trailing_ws {
        0
    } else {
        let bytes = line_to_pos.as_bytes();
        let mut i = bytes.len();
        while i > 0 && !bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        bytes.len() - i
    };

    Some(CompleterContext {
        cmd,
        current_word,
        prev_word,
        literal_len,
    })
}

/// 解析补全脚本 stdout 为候选列表（纯函数，便于单测）。
///
/// 切分规则：
/// - 按 `\n` 切分；
/// - 每行 `trim_end_matches('\r')` 容忍 CRLF；
/// - 过滤空行（`is_empty()`，纯空白行保留）；
/// - 零有效候选 → `None`（与脚本失败统一映射）。
pub(super) fn parse_completer_stdout(stdout: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for line in stdout.split('\n') {
        let line = line.trim_end_matches('\r');
        if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 执行已注册的补全脚本，返回 stdout 解析后的全部候选；任何异常返回 `None`。
///
/// argv 契约：`argv[1]=cmd`, `argv[2]=current_word`, `argv[3]=prev_word`。
///
/// env 契约（与 bash `complete -C` 对齐）：
/// - `COMP_LINE`  = 整行命令字面（无尾随换行）
/// - `COMP_POINT` = 光标在 COMP_LINE 中的零基字节索引
/// - 仅子进程可见（`Command::env` 链式追加，不调用 `std::env::set_var`）
///
/// 失败契约（脚本异常一律静默 no-op）：
/// - spawn 失败 / 非零退出 / stdout 非 UTF-8 / 零有效行 → None
fn run_completer_script(
    path: &str,
    cmd: &str,
    current_word: &str,
    prev_word: &str,
    comp_line: &str,
    comp_point: usize,
) -> Option<Vec<String>> {
    let output = Command::new(path)
        .arg(cmd)
        .arg(current_word)
        .arg(prev_word)
        .env("COMP_LINE", comp_line)
        .env("COMP_POINT", comp_point.to_string())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = std::str::from_utf8(&output.stdout).ok()?;
    parse_completer_stdout(stdout)
}

/// 命令级脚本补全总入口：组合 ctx + 脚本调用 + 双 TAB 状态机。
///
/// 行为分支：
/// - 脚本失败 / 0 候选 → 静默 no-op + 清状态（脚本噪音不污染交互）
/// - 1 候选 → 替换 `[start, pos)` 为 `<text> `；该轮节奏终结
/// - ≥2 候选 + LCP 可扩展 → 替换为 LCP（无尾空格），节奏作废
/// - ≥2 候选 + LCP 不可扩展 → 双 TAB 节奏：首次 BEL，二次列出 + 重画
pub(super) fn complete_with_script(
    helper: &ShellHelper,
    line: &str,
    pos: usize,
    path: &str,
    ctx: &CompleterContext,
) -> Result<(usize, Vec<Pair>)> {
    // 命令级脚本分支被触发：清掉对侧两套状态机
    helper.last_tab_prefix.set(None);
    helper.last_tab_arg_key.set(None);

    // 字面对齐校验：literal_len 理论上严格 ≤ pos；越界即按 no-op 退避。
    if ctx.literal_len > pos {
        return Ok((pos, Vec::new()));
    }
    let start = pos - ctx.literal_len;

    match run_completer_script(path, &ctx.cmd, &ctx.current_word, &ctx.prev_word, line, pos) {
        Some(mut names) if names.len() == 1 => {
            // 单候选：替换 [start, pos) 为 `<text> `
            helper.last_tab_script_key.set(None);
            let text = names.pop().unwrap();
            let pair = Pair {
                display: text.clone(),
                replacement: format!("{} ", text),
            };
            Ok((start, vec![pair]))
        }
        Some(mut names) => {
            // 多候选：先排序再 LCP 扩展
            names.sort();
            let lcp = longest_common_prefix(&names[0], names.last().unwrap()).to_string();
            if lcp.len() > ctx.current_word.len() {
                helper.last_tab_script_key.set(None);
                let pair = Pair {
                    display: lcp.clone(),
                    replacement: lcp,
                };
                return Ok((start, vec![pair]));
            }

            // 双 TAB 状态机
            let current_key = (
                ctx.cmd.clone(),
                ctx.current_word.clone(),
                ctx.prev_word.clone(),
            );
            let prev = helper.last_tab_script_key.take();
            let same_as_prev = prev.as_ref() == Some(&current_key);
            if same_as_prev {
                let joined = names.join("  ");
                print!("\n{}\n$ {}", joined, &line[..pos]);
                let _ = io::stdout().flush();
            } else {
                print!("\x07");
                let _ = io::stdout().flush();
                helper.last_tab_script_key.set(Some(current_key));
            }
            Ok((pos, Vec::new()))
        }
        // 脚本异常 / 零候选：静默 no-op + 清状态
        None => {
            helper.last_tab_script_key.set(None);
            Ok((pos, Vec::new()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- extract_completer_context 用例 ----

    fn ctx(line: &str) -> Option<(String, String, String, usize)> {
        extract_completer_context(line).map(
            |CompleterContext { cmd, current_word, prev_word, literal_len }| {
                (cmd, current_word, prev_word, literal_len)
            },
        )
    }

    #[test]
    fn ctx_cmd_only_trailing_space() {
        assert_eq!(
            ctx("docker "),
            Some(("docker".to_string(), String::new(), "docker".to_string(), 0))
        );
    }

    #[test]
    fn ctx_cmd_only_multispace() {
        assert_eq!(
            ctx("docker   "),
            Some(("docker".to_string(), String::new(), "docker".to_string(), 0))
        );
        assert_eq!(
            ctx("docker\t"),
            Some(("docker".to_string(), String::new(), "docker".to_string(), 0))
        );
    }

    #[test]
    fn ctx_cmd_with_partial_first_arg() {
        assert_eq!(
            ctx("git rem"),
            Some(("git".to_string(), "rem".to_string(), "git".to_string(), 3))
        );
    }

    #[test]
    fn ctx_cmd_with_complete_arg_trailing_space() {
        assert_eq!(
            ctx("git remote "),
            Some((
                "git".to_string(),
                String::new(),
                "remote".to_string(),
                0
            ))
        );
    }

    #[test]
    fn ctx_cmd_with_complete_arg_and_partial_next() {
        assert_eq!(
            ctx("git remote set"),
            Some((
                "git".to_string(),
                "set".to_string(),
                "remote".to_string(),
                3
            ))
        );
    }

    #[test]
    fn ctx_three_args_partial_last() {
        assert_eq!(
            ctx("git remote set foo"),
            Some((
                "git".to_string(),
                "foo".to_string(),
                "set".to_string(),
                3
            ))
        );
    }

    #[test]
    fn ctx_leading_whitespace() {
        assert_eq!(
            ctx("  docker "),
            Some(("docker".to_string(), String::new(), "docker".to_string(), 0))
        );
        assert_eq!(
            ctx("  git remote set"),
            Some((
                "git".to_string(),
                "set".to_string(),
                "remote".to_string(),
                3
            ))
        );
    }

    #[test]
    fn ctx_no_whitespace_returns_none() {
        assert_eq!(ctx("docker"), None);
        assert_eq!(ctx("d"), None);
        assert_eq!(ctx(""), None);
    }

    #[test]
    fn ctx_pure_whitespace_no_cmd() {
        let r = ctx("   ");
        if let Some((cmd, _, _, _)) = r {
            assert!(cmd.is_empty(), "cmd must be empty for pure whitespace");
        }
    }

    #[test]
    fn ctx_unclosed_quote_returns_none() {
        assert_eq!(ctx("cat 'unclosed"), None);
        assert_eq!(ctx("cat \"unclosed"), None);
    }

    #[test]
    fn ctx_literal_len_excludes_trailing_whitespace() {
        let (_, _, _, l) = ctx("git remote ").unwrap();
        assert_eq!(l, 0);
        let (_, _, _, l) = ctx("git ").unwrap();
        assert_eq!(l, 0);
    }

    #[test]
    fn ctx_literal_len_counts_trailing_non_ws_bytes() {
        let (_, _, _, l) = ctx("git remote set").unwrap();
        assert_eq!(l, 3);
        let (_, _, _, l) = ctx("a bcdef").unwrap();
        assert_eq!(l, 5);
    }

    #[test]
    fn ctx_stage_ep2_git_pu_argv_contract() {
        // Stage EP2 主例：argv[1]=cmd="git"，argv[2]=current="pu"，argv[3]=prev="git"
        assert_eq!(
            ctx("git pu"),
            Some(("git".to_string(), "pu".to_string(), "git".to_string(), 2))
        );
    }

    // ---- parse_completer_stdout：多候选解析纯函数 ----

    #[test]
    fn parse_single_line_no_trailing_newline() {
        assert_eq!(parse_completer_stdout("add"), Some(vec!["add".to_string()]));
    }

    #[test]
    fn parse_single_line_with_trailing_newline() {
        assert_eq!(parse_completer_stdout("add\n"), Some(vec!["add".to_string()]));
    }

    #[test]
    fn parse_multi_lines_lf() {
        assert_eq!(
            parse_completer_stdout("add\ncommit\npush\n"),
            Some(vec![
                "add".to_string(),
                "commit".to_string(),
                "push".to_string()
            ])
        );
        assert_eq!(
            parse_completer_stdout("add\ncommit\npush"),
            Some(vec![
                "add".to_string(),
                "commit".to_string(),
                "push".to_string()
            ])
        );
    }

    #[test]
    fn parse_multi_lines_crlf() {
        assert_eq!(
            parse_completer_stdout("add\r\ncommit\r\npush\r\n"),
            Some(vec![
                "add".to_string(),
                "commit".to_string(),
                "push".to_string()
            ])
        );
    }

    #[test]
    fn parse_filters_empty_lines() {
        assert_eq!(
            parse_completer_stdout("\nadd\n\ncommit\n\r\npush\n\n"),
            Some(vec![
                "add".to_string(),
                "commit".to_string(),
                "push".to_string()
            ])
        );
    }

    #[test]
    fn parse_all_empty_returns_none() {
        assert_eq!(parse_completer_stdout(""), None);
        assert_eq!(parse_completer_stdout("\n"), None);
        assert_eq!(parse_completer_stdout("\r\n\r\n"), None);
    }
}
