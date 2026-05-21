//! 语法层：在 token 序列基础上识别 6 类重定向操作符，组装出结构化命令。
//!
//! 详见父模块 [`crate::parser`] 头注释中关于 stdout/stderr 正交性的设计说明。
//!
//! 本模块同时承担 pipeline 解析：[`parse_pipeline`] 按引号外 `|` token 切分 N 段命令，
//! 每段独立保留 `>` / `>>` / `2>` / `2>>` 语义；末尾 `&` 仅作用于整条 pipeline。
//! [`parse`] 保留为单命令兼容 wrapper（内部调 `parse_pipeline` 后断言 stages.len()==1）。

use std::collections::HashMap;

use super::tokenize::tokenize;
use super::ParseError;

/// 结构化的一条命令：去除重定向元信息后的 argv，加上可选的 stdout / stderr 重定向目标
/// 及对应的「截断 / 追加」模式标志。
///
/// 当前阶段支持 stdout（`>` / `1>` / `>>` / `1>>`）与 stderr（`2>` / `2>>`）重定向；
/// 后续阶段可在此扩展 stdin 等字段而不需调整 [`tokenize`] 与上层 REPL 的契约。
///
/// # Examples
///
/// ```ignore
/// // `ls -la > out`
/// ParsedCommand {
///     argv: vec!["ls".into(), "-la".into()],
///     stdout_redirect: Some("out".into()),
///     stdout_append: false,
///     stderr_redirect: None,
///     stderr_append: false,
///     background: false,
/// }
/// // `cmd 2>> err.log &`
/// ParsedCommand {
///     argv: vec!["cmd".into()],
///     stdout_redirect: None,
///     stdout_append: false,
///     stderr_redirect: Some("err.log".into()),
///     stderr_append: true,
///     background: true,
/// }
/// ```
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
    pub stderr_append: bool,
    /// 末尾 `&` 触发的后台执行标志。
    ///
    /// **pipeline 时代语义**：本字段仅在 [`parse`]（单命令 wrapper）路径下由 `parse_pipeline`
    /// 的 `Pipeline.background` 回填——pipeline 时代后台标志的权威是 [`Pipeline::background`]，
    /// 单命令路径为保持既有调用方零改动而把同一布尔回写到 `ParsedCommand.background`。
    /// `parse_pipeline` 直接构造的每段 `ParsedCommand.background` 固定为 `false`。
    pub background: bool,
}

/// 一条 pipeline：N 段命令 + 整条 pipeline 是否后台运行。
///
/// `stages.len() >= 1` 总成立——单条命令也表示为 `Pipeline { stages: vec![cmd], .. }`，
/// 由上层 REPL 通过 `stages.len()` 判断走单命令快速路径还是多段串联路径。
///
/// `background` 仅作用于整条 pipeline（末尾 `&`）；段内 `&` token（如 `cmd1 & | cmd2`）
/// 不在本阶段语义内——`&` 在每段子序列内若出现于末尾会被剥离，与单命令路径行为一致，
/// 但因为 `|` 切分逻辑把 `&` 留在前一段子序列内，行为已知简化。
///
/// # Examples
///
/// ```ignore
/// // `cat file | wc -l`
/// Pipeline {
///     stages: vec![ParsedCommand { argv: vec!["cat".into(), "file".into()], .. },
///                  ParsedCommand { argv: vec!["wc".into(), "-l".into()], .. }],
///     background: false,
/// }
/// // `sleep 30 &`（单段 pipeline + 后台）
/// Pipeline {
///     stages: vec![ParsedCommand { argv: vec!["sleep".into(), "30".into()], .. }],
///     background: true,
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub stages: Vec<ParsedCommand>,
    pub background: bool,
}

/// 把一个子 token 序列（不含 `|` 与末尾 `&`）扫描为 [`ParsedCommand`]。
///
/// 单次线性扫描识别 6 类重定向操作符：
/// `>` / `1>` / `>>` / `1>>`（stdout）与 `2>` / `2>>`（stderr）。把紧随其后的 token
/// 作为对应 `*_redirect` 字段，并按操作符是否含 `>>` 设置 `*_append` 标志。
///
/// 错误：若任一重定向操作符后无下一 token，返回 [`ParseError::MissingRedirectTarget`]。
/// 重复 / 混用同向重定向（如 `> a >> b`、`>> a > b`、`2> e1 2>> e2`）取**最后一次**为准——
/// append 标志也跟随最后一次的操作符形式更新，与 bash 行为一致。
///
/// `background` 字段固定置为 `false`：后台标记是 pipeline 级别属性，由 [`parse_pipeline`]
/// 在 [`Pipeline::background`] 上维护；单命令 [`parse`] wrapper 完成后再回填到本字段。
fn collect_redirects(tokens: Vec<String>) -> Result<ParsedCommand, ParseError> {
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
        background: false,
    })
}

/// 把一行输入解析为 [`Pipeline`]：N 段 [`ParsedCommand`] + 整条 pipeline 后台标志。
///
/// 处理步骤：
/// 1. [`tokenize`] 输出扁平 token 序列；
/// 2. 若末尾 token 严格为 `"&"`，pop 之并置 `Pipeline.background = true`；
/// 3. 按 `"|"` token 切分剩余序列为多个子序列；
/// 4. 空子序列（首/末/中间空）→ [`ParseError::EmptyPipelineSegment`]；
/// 5. 每个子序列调用 [`collect_redirects`] 识别重定向，组装为 [`ParsedCommand`]。
///
/// 单条命令（无 `|`）：返回 `Pipeline { stages: vec![single], background }`，上层 REPL
/// 统一处理 `len() == 1` 走快速路径。
///
/// 已知限制：tokenize 输出的扁平 token 不携带类型标签，故 `echo "|"`（双引号内字面 `|`）
/// 在 token 层与 pipeline 分隔符无区别——但本阶段 tokenize 内部对引号态守护严格，
/// `|` 仅在 Normal 态被识别为独立 token，引号内字面 `|` 被合并进引号 token 内不会出现
/// 为单独 `"|"`，故 pipeline 切分不会把字面量误识别为分隔符。
///
/// # Examples
///
/// ```ignore
/// use std::collections::HashMap;
/// let vars = HashMap::new();
///
/// // 单命令
/// let p = parse_pipeline("ls -la", &vars).unwrap();
/// assert_eq!(p.stages.len(), 1);
/// assert!(!p.background);
///
/// // 多段 pipeline
/// let p = parse_pipeline("cat file | wc -l", &vars).unwrap();
/// assert_eq!(p.stages.len(), 2);
///
/// // 重定向 + 后台
/// let p = parse_pipeline("sleep 30 > log &", &vars).unwrap();
/// assert!(p.background);
/// assert_eq!(p.stages[0].stdout_redirect.as_deref(), Some("log"));
/// ```
pub fn parse_pipeline(
    input: &str,
    vars: &HashMap<String, String>,
) -> Result<Pipeline, ParseError> {
    let mut tokens = tokenize(input, vars)?;
    // 先识别并剥离末尾 `&`：本阶段 `&` 仅作为「pipeline 整体后台」标志。
    let background = matches!(tokens.last().map(|s| s.as_str()), Some("&"));
    if background {
        tokens.pop();
    }

    // 按 `"|"` token 切分子序列。
    // 注意：tokenize 内部 Normal 态 `|` 永远作为单字符独立 token push（参见
    // tokenize.rs 中 `|` 分支），故 token == "|" 是 pipeline 分隔符的可靠判据。
    let mut stages: Vec<ParsedCommand> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut has_pipe = false;
    for tok in tokens.into_iter() {
        if tok == "|" {
            has_pipe = true;
            if current.is_empty() {
                // 首段为空（开头 `|`）或连续 `||` 中间空段
                return Err(ParseError::EmptyPipelineSegment);
            }
            stages.push(collect_redirects(std::mem::take(&mut current))?);
        } else {
            current.push(tok);
        }
    }
    // 处理最后一段：含 `|` 但末段空（如 `ls |`）→ 错误；
    // 无 `|` 而 current 空 → 空输入或仅 `&`，stages 为空——保持 stages 为空交由
    // 上层 REPL 与既有「argv 空跳过」逻辑兼容。
    if has_pipe {
        if current.is_empty() {
            return Err(ParseError::EmptyPipelineSegment);
        }
        stages.push(collect_redirects(current)?);
    } else if !current.is_empty() {
        stages.push(collect_redirects(current)?);
    } else {
        // 完全空（如仅 `&` 或空输入）：仍返回单段空 ParsedCommand，
        // 与既有 `parse` 行为兼容（上层判 argv.is_empty() 跳过）。
        stages.push(ParsedCommand {
            argv: Vec::new(),
            stdout_redirect: None,
            stdout_append: false,
            stderr_redirect: None,
            stderr_append: false,
            background: false,
        });
    }

    Ok(Pipeline { stages, background })
}

/// 兼容 wrapper：把输入解析为单条 [`ParsedCommand`]。
///
/// 内部调用 [`parse_pipeline`]：
/// - 若 `stages.len() == 1`：取唯一段，把 `Pipeline.background` 回填到 `ParsedCommand.background`
///   后返回——既有单元测试与未来 API 调用完全兼容。
/// - 若 `stages.len() > 1`：返回 [`ParseError::EmptyPipelineSegment`] 兜底（REPL 已切到
///   [`parse_pipeline`] 直接调用，本分支仅作为防御性回退）。
///
/// 保留此函数以避免大量既有 parser 单元测试迁移成本，并对外暴露稳定 API。REPL
/// 主路径（`main.rs`）不再调用本函数——只在 `#[cfg(test)]` 路径与未来外部调用方使用，
/// 故对非 test 编译加 `#[allow(dead_code)]` 抑制「未使用」警告。
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse(
    input: &str,
    vars: &HashMap<String, String>,
) -> Result<ParsedCommand, ParseError> {
    let mut pipeline = parse_pipeline(input, vars)?;
    if pipeline.stages.len() == 1 {
        let mut cmd = pipeline.stages.pop().expect("len == 1");
        cmd.background = pipeline.background;
        Ok(cmd)
    } else {
        // 多段 pipeline 经此 wrapper 调用属于误用——既有单命令调用方收到此错误
        // 即可定位到「应改用 parse_pipeline」。
        Err(ParseError::EmptyPipelineSegment)
    }
}
