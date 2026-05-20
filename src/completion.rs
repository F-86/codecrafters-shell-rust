//! Tab 自动补全：仅在「首词」位置按前缀匹配 builtin 命令名。
//!
//! 设计要点：
//! - 候选源：`builtins::BUILTINS`（单一事实来源，新增 builtin 自动获得补全）。
//! - 触发条件：`line[..pos]` 不含空白；一旦进入参数区不做 builtin 补全，避免误补。
//! - 替换语义：`Pair.replacement = "<name> "`，末尾带空格，满足题面 "Add a trailing space"。
//!   起始替换位置固定为 0（首词从行首开始；rustyline 单独绘制提示符，不计入 line）。
//! - rustyline 要求 helper 同时实现 Completer/Hinter/Highlighter/Validator + Helper（marker）。
//!   非补全 trait 走默认实现即可。

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result};

use crate::builtins::BUILTINS;

/// rustyline 的 Helper 聚合体；当前仅承载 builtin 补全逻辑，无内部状态。
pub struct ShellHelper;

impl ShellHelper {
    pub fn new() -> Self {
        ShellHelper
    }
}

impl Completer for ShellHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>)> {
        // 仅在首词位置触发：光标左侧子串不含任何空白即视为仍在键入命令名。
        // 例：`ech` -> 触发；`echo he` -> 不触发；`  ech` -> 不触发（保守不补，避免引入额外语义）。
        let prefix = &line[..pos];
        if prefix.chars().any(|c| c.is_whitespace()) {
            return Ok((pos, Vec::new()));
        }

        // 前缀匹配 BUILTINS；replacement 末尾追加空格以满足"补全后立即可键入参数"。
        let candidates: Vec<Pair> = BUILTINS
            .iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| Pair {
                display: (*name).to_string(),
                replacement: format!("{} ", name),
            })
            .collect();

        // 替换起点为 0：首词从行首开始。
        Ok((0, candidates))
    }
}

// 以下三个 trait 走默认实现：当前阶段不做提示 / 高亮 / 校验，
// 仅为满足 Helper 组合约束。
impl Hinter for ShellHelper {
    type Hint = String;
}
impl Highlighter for ShellHelper {}
impl Validator for ShellHelper {}
impl Helper for ShellHelper {}
