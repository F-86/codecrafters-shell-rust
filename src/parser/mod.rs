//! 命令行词法分析器（tokenizer）+ 命令结构化解析器（parser）。
//!
//! 当前支持单引号、双引号、「引号外反斜杠转义」、`>` / `1>` 重定向操作符
//! 与 `$VAR` 变量展开语义：
//! - 单引号内任何字符（含空格、`$`、`*`、`~`、`"`、`\`、`>`、Tab 等）按字面量保留；
//! - 双引号内大部分字符按字面量保留（含空格、单引号、`*`、`;`、`>` 等）；`\` 仅对
//!   `"`、`\`、`$`、`` ` `` 这 4 个字符触发转义并吃掉自身，其他字符前 `\` 按字面量
//!   保留；`$NAME` 触发变量展开（命中替换为值，未命中替换为空串），`\$` 提前消费
//!   `$` 作为字面 push，下一轮主循环跳过展开分支，天然实现 `\$VAR` 不展开；
//! - 引号外 `\X` 移除 `X` 的特殊含义并按字面量保留 `X`，反斜杠本身被丢弃，
//!   适用于任意下一字符（含空白、`'`、`"`、`$`、`*`、`>` 等及普通字母）；行尾孤立 `\`
//!   视为语法错误（`TrailingBackslash`）；
//! - 引号外 `$NAME` 触发变量展开：合法 NAME（`^[A-Za-z_][A-Za-z0-9_]*`）贪婪扫描
//!   到第一个非合法字符为止，命中 `vars` 替换为值、未命中替换为空串；`$` 后非
//!   合法首字符（如 `$1abc` / `$-` / `$<空白>` / 行尾孤立 `$`）按字面量保留 `$`；
//! - 引号外与双引号内还支持 `${NAME}` 大括号形式：扫描到 `}` 闭合后对内部字符串
//!   做整串 NAME 校验（同 `$NAME` 字符集，与 `is_name_start`/`is_name_cont` 同源），
//!   命中替换为值、未命中替换为空串。大括号边界明确，可让 `${X}end` 拼接字面 `end`
//!   而无需空白分隔。错误形式：内部 NAME 非法（空 `${}`、首字符非法 `${1abc}`、
//!   含非 NAME 字符 `${X-Y}` 等）→ `BadSubstitution`；行尾未见闭合 `}` →
//!   `UnterminatedBraceExpansion`。`\${X}` 在 Normal 与双引号态均复用反斜杠路径——
//!   `$` 被先消费为字面 push，下一轮主循环看到 `{` 字面字符，自然得到 `${X}` 字面输出；
//! - 引号外连续空白作为 token 分隔符并被折叠；
//! - 任意相邻的引号串 / 空引号 / 裸字符串 / 转义字符可无缝拼接成同一个 argument；
//! - 引号外 `>` 与 `1>` / `2>` 被识别为独立 token：当且仅当 `>` 紧贴在裸字符 `1` 或 `2`
//!   之后（中间无空白、无引号、无转义）时分别合并为单 token `"1>"` / `"2>"`，其余情形
//!   `>` 单独成 token；引号内 `>` 仍按字面量。
//! - 引号外连续两个 `>` 紧贴时合并为追加重定向操作符：`">>"` / `"1>>"` / `"2>>"`。
//!   合并要求两个 `>` 之间无空白、无引号、无转义；引号内 `>>` 仍按字面量。
//! - 引号外 `&` 被识别为独立 token（无论前是否有空白），与 `>` 切分规则对称：
//!   `sleep 30 &` 与 `sleep 30&` 均切出 `["sleep", "30", "&"]`。引号内 `&` 与引号外
//!   `\&` 在词法层按字面量保留为字符 `&`——但**当字面量 `&` 恰好独立成 token**
//!   时（如 `echo "&"` / `echo \&`），token 字符串与操作符形态完全相同，parse 层
//!   无法区分（已知简化，详见 [`ParsedCommand::background`] 字段 doc）。
//!
//! 上层 `parse` 函数在 `tokenize` 输出基础上识别 6 类重定向操作符：
//! - stdout 截断：`>` / `1>`（等价）
//! - stdout 追加：`>>` / `1>>`（等价）
//! - stderr 截断：`2>`
//! - stderr 追加：`2>>`
//!
//! 把其后第一个 token 作为对应目标，剩余 token 作为 argv，组装出 [`ParsedCommand`]。
//!
//! 此外，若 token 序列末尾恰好是单独的 `"&"`，parse 层会先 pop 之并把
//! [`ParsedCommand::background`] 置为 `true`，由 REPL 在外部命令分支以 `spawn` 不
//! `wait` 的方式启动子进程并打印 `[<job>] <pid>` 通知。非末尾位置的 `&` token 按
//! 普通字面量参数留在 argv，与 bash 复合后台分隔语义不同（本阶段简化）。
//!
//! **stdout 与 stderr 重定向完全正交**：两者拥有独立的 `*_redirect` + `*_append`
//! 字段，互不影响。任一流未被显式重定向时，对应字段为 `None`，由上层 REPL 把外部
//! 命令的该流设为 [`std::process::Stdio::inherit`]（直通终端）。这是「`ls foo >> out`
//! 时 stderr 仍在终端可见」「`ls foo 2>> err` 时 stdout 仍在终端可见」的语义根因。
//!
//! ## 模块组织
//!
//! 本模块拆分为三个子模块 + 一个测试文件：
//! - [`tokenize`]：词法层，把输入字符串切分为 token 序列；
//! - [`parse`]：语法层，在 token 序列基础上识别 6 类重定向操作符；
//! - `tests`（仅 `#[cfg(test)]`）：单元测试集中存放，可访问私有 API。
//!
//! 对外暴露的 4 个符号通过本文件的 `pub use` 重导出，保持
//! `parser::parse` / `parser::tokenize` / `parser::ParsedCommand` / `parser::ParseError`
//! 这 4 条访问路径稳定，与拆分前一致。

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
