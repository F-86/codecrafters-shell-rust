//! 三分支共享的纯函数：LCP / 目录扫描 / 路径分类 / Pair 格式化。
//!
//! 这些函数无 `&self` 依赖，便于单测；`MatchKind` 枚举与 `format_arg_completion`
//! 在「目录尾 `/` 不加空格、文件加尾空格」的语义下被参数与脚本分支共用。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rustyline::completion::Pair;

use crate::parser::tokenize;

/// 返回 `a` 与 `b` 的最长公共前缀切片（按 UTF-8 char 边界安全截取）。
///
/// 实现说明：用 `char_indices` 同步遍历两串，遇到首个不一致字符即按其字节起点切片；
/// 若一串先耗尽则较短串自身即为 LCP。返回值借自 `a`，生命周期与 `a` 一致。
pub(crate) fn longest_common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let mut ai = a.char_indices();
    let mut bi = b.char_indices();
    loop {
        match (ai.next(), bi.next()) {
            (Some((i, ca)), Some((_, cb))) if ca == cb => {
                let _ = i;
            }
            (Some((i, _)), Some(_)) => return &a[..i],
            (None, _) => return a,
            (Some((i, _)), None) => return &a[..i],
        }
    }
}

/// 从「光标左侧子串」中提取参数位置补全的前缀。
///
/// 输入约定：调用方已确认 `line_to_pos` 至少含一个空白（即处于参数区，不再是命令名）。
///
/// 返回值：
/// - `Some(String::new())`：末尾是空白，用户在 token 边界按 TAB；语义为"列出 cwd
///   全部 entry"（空 prefix 与任何叶子名 `starts_with("")` 永真，由调用方统一处理）。
/// - `Some(prefix)`：末尾非空白，返回 tokenize 后的最后一个 token —— 用户正在键入
///   的"逻辑前缀"。tokenize 折叠了空白并按引号语义剥离。
/// - `None`：tokenize 失败（未闭合引号、行尾孤立反斜杠等）；调用方按静默 no-op 处理。
pub(crate) fn extract_arg_prefix(line_to_pos: &str) -> Option<String> {
    if line_to_pos.chars().next_back().map_or(true, |c| c.is_whitespace()) {
        return Some(String::new());
    }
    // tokenize 第二参 vars：补全期不接入 shell 变量后端——用户键入 `$V<TAB>` 时
    // 期望补全的是变量名本身，而不是先把 `$V` 展开成空串再据空 prefix 列出全部 entry。
    // 本阶段 tester 不覆盖 `$VAR` 与补全的交互，故传空表保持现状。
    let tokens = tokenize(line_to_pos, &HashMap::new()).ok()?;
    tokens.into_iter().last()
}

/// 把参数 token 切分为 (dir_part, name_prefix)，供嵌套路径补全使用。
///
/// 切分规则：以 token 中**最后一个 `/`** 为切点；`dir_part` 始终包含尾 `/`。
/// 不含 `/` 时退化为 `("", token)`，让调用方按 cwd 场景处理。
///
/// 例：
/// - `"f"`         → `("", "f")`
/// - `"path/to/f"` → `("path/to/", "f")`
/// - `"path/to/"`  → `("path/to/", "")`
/// - `"/etc/h"`    → `("/etc/", "h")`
/// - `""`          → `("", "")`
pub(crate) fn split_dir_and_name(token: &str) -> (&str, &str) {
    match token.rfind('/') {
        Some(idx) => token.split_at(idx + 1),
        None => ("", token),
    }
}

/// 扫描指定目录，返回所有以 `name_prefix` 字面开头的 entry 叶子名（**不含 dir 前缀**）。
///
/// - I/O 失败（不存在 / 非目录 / 权限）静默返回空 Vec：补全是交互路径，写错误日志
///   会污染用户输入区。
/// - 不区分 file/dir；调用方负责拼回 dir 前缀形成完整路径。
/// - 复杂度 O(N)，N 为目标目录条目数；TAB 是低频交互，不做缓存。
pub(crate) fn match_files_in_dir(dir: &Path, name_prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let iter = match fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in iter.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(name_prefix) {
            out.push(name.into_owned());
        }
    }
    out
}

/// 单匹配 entry 的类型分类。
///
/// 用枚举而非 `bool` 标志：调用点 `format_arg_completion(&full, MatchKind::Directory)`
/// 比 `format_arg_completion(&full, true)` 自解释；未来扩展（如 `Symlink`）时
/// 函数签名不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchKind {
    File,
    Directory,
}

/// 判定 `path` 是文件还是目录（跟随 symlink，与 bash/zsh/fish 一致）。
///
/// `fs::metadata` 跟随 symlink 取最终目标的元数据（指向目录的 symlink 也加 `/`）。
/// 任何 I/O 失败一律退化为 `MatchKind::File`——加尾空格是语义安全的退化。
pub(crate) fn classify_path(path: &Path) -> MatchKind {
    match fs::metadata(path) {
        Ok(m) if m.is_dir() => MatchKind::Directory,
        _ => MatchKind::File,
    }
}

/// 把单匹配的完整路径与类型格式化为 rustyline 的 `Pair`。
///
/// - `MatchKind::Directory` → `replacement = "{full}/"`，**无**尾空格，便于继续 TAB 进入下一层。
/// - `MatchKind::File`      → `replacement = "{full} "`，加尾空格。
pub(crate) fn format_arg_completion(full: &str, kind: MatchKind) -> Pair {
    match kind {
        MatchKind::Directory => Pair {
            display: format!("{}/", full),
            replacement: format!("{}/", full),
        },
        MatchKind::File => Pair {
            display: full.to_string(),
            replacement: format!("{} ", full),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- longest_common_prefix ----

    #[test]
    fn lcp_basic() {
        assert_eq!(longest_common_prefix("xyz_foo", "xyz_foo_bar"), "xyz_foo");
        assert_eq!(longest_common_prefix("xyz_foo_bar", "xyz_foo_bar_baz"), "xyz_foo_bar");
        assert_eq!(longest_common_prefix("xyz_bar", "xyz_quz"), "xyz_");
        assert_eq!(longest_common_prefix("abc", "xyz"), "");
        assert_eq!(longest_common_prefix("", "abc"), "");
        assert_eq!(longest_common_prefix("abc", ""), "");
        assert_eq!(longest_common_prefix("same", "same"), "same");
    }

    // ---- Stage EP3：LCP 扩展决策的边界用例 ----

    #[test]
    fn lcp_stage_ep3_strictly_longer_than_current_word() {
        let lcp_str = longest_common_prefix("checkout", "cherry-pick");
        assert_eq!(lcp_str, "che");
        assert!(lcp_str.len() > "c".len());
    }

    #[test]
    fn lcp_stage_ep3_equal_to_current_word_no_extend() {
        let lcp_str = longest_common_prefix("apply", "append");
        assert_eq!(lcp_str, "app");
        assert!(!(lcp_str.len() > "app".len()));
    }

    #[test]
    fn lcp_stage_ep3_empty_lcp_no_extend() {
        let lcp_str = longest_common_prefix("foo", "bar");
        assert_eq!(lcp_str, "");
        assert!(!(lcp_str.len() > "f".len()));
    }

    // ---- extract_arg_prefix ----

    #[test]
    fn extract_prefix_normal() {
        assert_eq!(extract_arg_prefix("cat re"), Some("re".to_string()));
        assert_eq!(extract_arg_prefix("xyz read"), Some("read".to_string()));
        assert_eq!(
            extract_arg_prefix("cat foo bar bz"),
            Some("bz".to_string())
        );
    }

    #[test]
    fn extract_prefix_trailing_space_returns_empty() {
        assert_eq!(extract_arg_prefix("cat re "), Some(String::new()));
        assert_eq!(extract_arg_prefix("cat "), Some(String::new()));
        assert_eq!(extract_arg_prefix("cat   "), Some(String::new()));
    }

    #[test]
    fn extract_prefix_tokenize_error_returns_none() {
        assert_eq!(extract_arg_prefix("cat 'unclosed"), None);
        assert_eq!(extract_arg_prefix("cat \"unclosed"), None);
        assert_eq!(extract_arg_prefix("cat foo\\"), None);
    }

    // ---- Stage: Completion in Any Argument Position ----

    #[test]
    fn extract_prefix_at_third_arg() {
        assert_eq!(
            extract_arg_prefix("ls bar/ foo x"),
            Some("x".to_string())
        );
    }

    #[test]
    fn extract_prefix_after_dir_arg() {
        assert_eq!(extract_arg_prefix("ls bar/ "), Some(String::new()));
    }

    #[test]
    fn extract_prefix_subsequent_with_prefix() {
        assert_eq!(extract_arg_prefix("ls bar/ f"), Some("f".to_string()));
    }

    // ---- split_dir_and_name ----

    #[test]
    fn split_no_slash() {
        assert_eq!(split_dir_and_name("f"), ("", "f"));
    }

    #[test]
    fn split_empty_token() {
        assert_eq!(split_dir_and_name(""), ("", ""));
    }

    #[test]
    fn split_relative_path() {
        assert_eq!(split_dir_and_name("path/to/f"), ("path/to/", "f"));
    }

    #[test]
    fn split_trailing_slash() {
        assert_eq!(split_dir_and_name("path/to/"), ("path/to/", ""));
    }

    #[test]
    fn split_absolute_path() {
        assert_eq!(split_dir_and_name("/etc/h"), ("/etc/", "h"));
    }

    #[test]
    fn split_multi_level() {
        assert_eq!(split_dir_and_name("a/b/c/d"), ("a/b/c/", "d"));
    }

    #[test]
    fn split_dir_and_name_isolated_arg() {
        // 裸名（无 '/'）→ dir_part 为空 → 扫描 CWD
        assert_eq!(split_dir_and_name("f"), ("", "f"));
    }

    // ---- match_files_in_dir：依赖 cargo 测试 cwd = crate root ----

    #[test]
    fn match_files_finds_unique_prefix() {
        let v = match_files_in_dir(std::path::Path::new("."), "Cargo.t");
        assert_eq!(v, vec!["Cargo.toml".to_string()]);
    }

    #[test]
    fn match_files_multi_match_returns_all() {
        let mut v = match_files_in_dir(std::path::Path::new("."), "Cargo.");
        v.sort();
        assert_eq!(
            v,
            vec!["Cargo.lock".to_string(), "Cargo.toml".to_string()]
        );
    }

    #[test]
    fn match_files_no_match_empty() {
        let v = match_files_in_dir(std::path::Path::new("."), "zzz_no_such_prefix_");
        assert!(v.is_empty());
    }

    #[test]
    fn match_files_nested_finds_entry() {
        // 拆分后 src/ 下不再有单文件 completion.rs，但有 builtins/mod.rs 等。
        // 改为断言 src/ 下能找到 main.rs（项目入口文件，恒存在）。
        let v = match_files_in_dir(std::path::Path::new("src"), "main");
        assert!(
            v.iter().any(|n| n == "main.rs"),
            "expected main.rs in {:?}",
            v
        );
    }

    #[test]
    fn match_files_nonexistent_dir_returns_empty() {
        let v = match_files_in_dir(
            std::path::Path::new("zzz_no_such_dir_xyz_qqq"),
            "",
        );
        assert!(v.is_empty());
    }

    // ---- classify_path ----

    #[test]
    fn classify_path_directory() {
        assert_eq!(
            classify_path(std::path::Path::new("src")),
            MatchKind::Directory
        );
    }

    #[test]
    fn classify_path_file() {
        assert_eq!(
            classify_path(std::path::Path::new("Cargo.toml")),
            MatchKind::File
        );
    }

    #[test]
    fn classify_path_missing_falls_back_to_file() {
        assert_eq!(
            classify_path(std::path::Path::new("zzz_no_such_path_qqq")),
            MatchKind::File
        );
    }

    // ---- format_arg_completion ----

    #[test]
    fn format_arg_completion_file_flat() {
        let p = format_arg_completion("foo.txt", MatchKind::File);
        assert_eq!(p.display, "foo.txt");
        assert_eq!(p.replacement, "foo.txt ");
    }

    #[test]
    fn format_arg_completion_directory_flat() {
        let p = format_arg_completion("project", MatchKind::Directory);
        assert_eq!(p.display, "project/");
        assert_eq!(p.replacement, "project/");
    }

    #[test]
    fn format_arg_completion_file_nested() {
        let p = format_arg_completion("path/to/foo.txt", MatchKind::File);
        assert_eq!(p.display, "path/to/foo.txt");
        assert_eq!(p.replacement, "path/to/foo.txt ");
    }

    #[test]
    fn format_arg_completion_directory_nested() {
        let p = format_arg_completion("pig/dog", MatchKind::Directory);
        assert_eq!(p.display, "pig/dog/");
        assert_eq!(p.replacement, "pig/dog/");
    }
}
