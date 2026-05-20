//! 语法层：在 token 序列基础上识别 6 类重定向操作符，组装出结构化命令。
//!
//! 详见父模块 [`crate::parser`] 头注释中关于 stdout/stderr 正交性的设计说明。

use super::tokenize::tokenize;
use super::ParseError;

/// 结构化的一条命令：去除重定向元信息后的 argv，加上可选的 stdout / stderr 重定向目标
/// 及对应的「截断 / 追加」模式标志。
///
/// 当前阶段支持 stdout（`>` / `1>` / `>>` / `1>>`）与 stderr（`2>` / `2>>`）重定向；
/// 后续阶段可在此扩展 stdin 等字段而不需调整 [`tokenize`] 与上层 REPL 的契约。
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedCommand {
    /// 命令名 + 参数（不含任何重定向操作符或目标文件 token）。
    /// 当输入是仅含空白或仅含重定向（如 `> out`）时，`argv` 可能为空——由上层 REPL 决定如何处理。
    pub argv: Vec<String>,
    /// stdout 重定向目标文件路径；`None` 表示未指定重定向。
    /// `>` / `1>` / `>>` / `1>>` 都填这一字段；模式由 `stdout_append` 区分。
    pub stdout_redirect: Option<String>,
    /// stdout 追加模式标志：`true` 表示用 `>>` / `1>>`（追加打开）；`false` 表示用 `>` / `1>`
    /// （截断打开）或未指定重定向。`stdout_redirect == None` 时该字段无意义，固定为 `false`。
    pub stdout_append: bool,
    /// stderr 重定向目标文件路径（`2>` / `2>>` 操作符）；`None` 表示未指定。
    /// 与 stdout_redirect 完全独立：一行中可同时出现 `> out 2>> err`，互不干扰。
    pub stderr_redirect: Option<String>,
    /// stderr 追加模式标志：`true` 表示用 `2>>`；`false` 表示用 `2>` 或未指定重定向。
    /// `stderr_redirect == None` 时该字段无意义，固定为 `false`。
    ///
    /// 与 `stdout_redirect` / `stdout_append` 完全正交：可单独追加 stderr 同时保留
    /// stdout 终端输出（如 `cmd 2>> err` —— stdout 仍 inherit 到终端），亦可同时设置
    /// 两套追加（如 `cmd >> out 2>> err`），两文件独立 append。
    pub stderr_append: bool,
}

/// 把一行输入解析为 [`ParsedCommand`]。
///
/// 内部先调用 [`tokenize`] 得到扁平 token 序列，再单次线性扫描识别 6 类重定向操作符：
/// `>` / `1>` / `>>` / `1>>`（stdout）与 `2>` / `2>>`（stderr）。把紧随其后的 token
/// 作为对应 `*_redirect` 字段，并按操作符是否含 `>>` 设置 `*_append` 标志。
///
/// 错误传播：tokenize 阶段的语法错误原样返回；若任一重定向操作符后无下一 token,
/// 返回 [`ParseError::MissingRedirectTarget`]。重复 / 混用同向重定向（如 `> a >> b`、
/// `>> a > b`、`2> e1 2>> e2`）取**最后一次**为准——append 标志也跟随最后一次的操作符
/// 形式更新，与 bash 行为一致。
pub fn parse(input: &str) -> Result<ParsedCommand, ParseError> {
    let tokens = tokenize(input)?;
    let mut argv: Vec<String> = Vec::with_capacity(tokens.len());
    let mut stdout_redirect: Option<String> = None;
    let mut stdout_append = false;
    let mut stderr_redirect: Option<String> = None;
    let mut stderr_append = false;
    let mut iter = tokens.into_iter();
    while let Some(tok) = iter.next() {
        // 归一化：`>` / `1>` 截断 stdout；`>>` / `1>>` 追加 stdout；
        //         `2>` 截断 stderr；`2>>` 追加 stderr。
        // 重复出现取最后一次（含 append 标志），与 bash 一致。
        match tok.as_str() {
            ">" | "1>" => match iter.next() {
                Some(target) => {
                    stdout_redirect = Some(target);
                    stdout_append = false;
                }
                None => return Err(ParseError::MissingRedirectTarget),
            },
            ">>" | "1>>" => match iter.next() {
                Some(target) => {
                    stdout_redirect = Some(target);
                    stdout_append = true;
                }
                None => return Err(ParseError::MissingRedirectTarget),
            },
            "2>" => match iter.next() {
                Some(target) => {
                    stderr_redirect = Some(target);
                    stderr_append = false;
                }
                None => return Err(ParseError::MissingRedirectTarget),
            },
            "2>>" => match iter.next() {
                Some(target) => {
                    stderr_redirect = Some(target);
                    stderr_append = true;
                }
                None => return Err(ParseError::MissingRedirectTarget),
            },
            _ => argv.push(tok),
        }
    }
    Ok(ParsedCommand {
        argv,
        stdout_redirect,
        stdout_append,
        stderr_redirect,
        stderr_append,
    })
}
