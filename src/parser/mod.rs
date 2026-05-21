//! 命令行词法 + 语法解析。
//!
//! 顶层入口：[`parse_pipeline`] —— 输入字符串 + shell 变量后端，输出 [`Pipeline`]
//! （`Vec<ParsedCommand>` + `background` 标志）或 [`ParseError`]。
//!
//! 语义覆盖：单/双引号、引号外反斜杠、`\$` 转义、`$VAR` / `${NAME}` 展开、
//! null word removal、6 类重定向（`>` / `1>` / `>>` / `1>>` / `2>` / `2>>`）、
//! `\|` pipeline 切分、尾 `&` 后台标志。完整语义表与决策见
//! [docs/DESIGN_DECISIONS.md#parser-architecture](../../docs/DESIGN_DECISIONS.md#parser-architecture)。
//!
//! ## 模块组织
//!
//! - [`mod@tokenize`] — 词法层：字符级状态机，单次线性扫描完成引号/转义/`$VAR` 展开/操作符识别
//! - [`parse`]    — 语法层：在 token 序列上识别重定向 + pipeline 切分
//! - `tests`      — 60+ 单元测试，集中存放可访问私有 API
//!
//! 对外通过 `pub use` 重导出 `parse_pipeline` / `Pipeline` / `ParsedCommand` / `ParseError`。


use std::fmt;

mod parse;
mod tokenize;

#[cfg(test)]
mod tests;

pub use parse::{parse_pipeline, ParsedCommand, Pipeline};
// `parse` 是单命令兼容 wrapper，仅供 parser 内部 `#[cfg(test)]` 单元测试调用——
// REPL 主路径已切到 `parse_pipeline`。对外仍 re-export 以保持 API 稳定（未来扩展
// 或外部嵌入测试可直接调用），并把「未使用」警告收敛到 parse.rs 的 `#[cfg_attr]` 上。
#[cfg(test)]
pub use parse::parse;
// `tokenize` 当前仅供 parser 内部 `parse` 与 `#[cfg(test)] mod tests` 使用，
// 但作为词法层稳定 API 仍对外暴露——若未来 main 或新模块需直接调用 tokenize
// （例如做语法高亮、补全），此 re-export 提供零成本入口。
#[allow(unused_imports)]
pub use tokenize::tokenize;
// NAME 字符级 helper：`builtins::is_valid_identifier` 与 `tokenize` 内部 `$VAR`
// 展开 NAME 扫描共享同一对函数，跨 stage 100% 同源。`pub(crate)` 限定可见性，
// 仅 crate 内部使用，不作为对外稳定 API。
pub(crate) use tokenize::{is_name_cont, is_name_start};

/// 词法分析阶段可能产生的错误。
///
/// 保留 enum 形式便于后续阶段（双引号内部分转义、变量展开等）继续扩展。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// 行末仍在单引号内部（缺少闭合 `'`）。
    UnterminatedSingleQuote,
    /// 行末仍在双引号内部（缺少闭合 `"`）。
    UnterminatedDoubleQuote,
    /// 行末为孤立反斜杠（无字符可转义）。
    TrailingBackslash,
    /// 任一重定向操作符（`>` / `1>` / `2>` / `>>` / `1>>` / `2>>`）后没有目标文件 token，
    /// 例如 `echo hello >`、`ls 2>`、`echo a >>`。
    MissingRedirectTarget,
    /// pipeline 中存在空段：开头 `|`（如 `| ls`）、末尾 `|`（如 `ls |`）、连续 `||`
    /// （如 `ls | | cat` 或 `a || b`）。本阶段不实现 `||` 逻辑 OR，连续 `|` 一律视为
    /// 空 stage 报错；未来如需支持 `||` 可在 tokenize 层先合并为独立 `"||"` token 再扩展。
    EmptyPipelineSegment,
    /// `${...}` 内部 NAME 非法：空 `${}`、首字符非法（如 `${1abc}`、`${-X}`、`${ }`）、
    /// 中间含非 NAME 字符（如 `${X-Y}`、`${X.Y}`、`${X Y}`）。与 bash `bad substitution`
    /// 错误语义一致，最严格立即报错（不做字面量降级）。
    BadSubstitution,
    /// `${` 行尾仍未见闭合 `}`，如 `echo ${X` / `echo ${`。与
    /// `UnterminatedSingleQuote` / `UnterminatedDoubleQuote` 风格一致：行内未闭合
    /// 一律视为语法错误，由 REPL 决定如何提示。
    UnterminatedBraceExpansion,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnterminatedSingleQuote => {
                write!(f, "syntax error: unterminated single quote")
            }
            ParseError::UnterminatedDoubleQuote => {
                write!(f, "syntax error: unterminated double quote")
            }
            ParseError::TrailingBackslash => {
                write!(f, "syntax error: trailing backslash")
            }
            ParseError::MissingRedirectTarget => {
                write!(f, "syntax error: missing redirect target")
            }
            ParseError::EmptyPipelineSegment => {
                write!(f, "syntax error: empty pipeline segment")
            }
            ParseError::BadSubstitution => {
                write!(f, "syntax error: bad substitution")
            }
            ParseError::UnterminatedBraceExpansion => {
                write!(f, "syntax error: unterminated brace expansion")
            }
        }
    }
}

impl std::error::Error for ParseError {}
