//! Tab 自动补全：仅在「首词」位置按前缀匹配 builtin 与 PATH 中的可执行文件。
//!
//! 设计要点：
//! - 候选源 1：`builtins::BUILTINS`（单一事实来源，新增 builtin 自动获得补全）。
//! - 候选源 2：`builtins::list_path_executables()`（启动期一次性扫描 PATH 并缓存到
//!   `ShellHelper.path_executables`，运行期不再重新扫描）。
//! - 触发条件：`line[..pos]` 不含空白；一旦进入参数区不做命令名补全，避免误补。
//! - 替换语义：`Pair.replacement = "<name> "`，末尾带空格，满足题面 "Add a trailing space"。
//!   起始替换位置固定为 0（首词从行首开始；rustyline 单独绘制提示符，不计入 line）。
//! - 去重：同名时 **builtin 优先**，PATH 同名忽略；PATH 内部不同目录的同名也只取首个，
//!   与 `find_in_path` 按 PATH 顺序解析的执行行为对齐。用 `HashSet<&str>` 跟踪已加入名字。
//! - rustyline 要求 helper 同时实现 Completer/Hinter/Highlighter/Validator + Helper（marker）。
//!   非补全 trait 走默认实现即可。

use std::collections::HashSet;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result};

use crate::builtins::{list_path_executables, BUILTINS};

/// rustyline 的 Helper 聚合体；承载 builtin 列表（编译期常量）+ PATH 可执行文件缓存。
///
/// `path_executables` 在 `new()` 中一次性填充，后续 TAB 触发只做内存遍历，
/// 牺牲对运行期 PATH 变化的感知换取更低补全延迟。
pub struct ShellHelper {
    path_executables: Vec<String>,
}

impl ShellHelper {
    pub fn new() -> Self {
        ShellHelper {
            path_executables: list_path_executables(),
        }
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

        let mut candidates: Vec<Pair> = Vec::new();
        // seen 跟踪已加入候选的命令名，实现「builtin 优先 + PATH 内部去重」；
        // 借用 &str 即可（builtin 是 &'static str，path 来自 self 的字段，
        // 生命周期均覆盖本函数返回前的使用范围）。
        let mut seen: HashSet<&str> = HashSet::new();

        // 第一阶段：builtin 前缀匹配候选（最高优先级，先入 seen）
        for name in BUILTINS {
            if name.starts_with(prefix) {
                seen.insert(*name);
                candidates.push(Pair {
                    display: (*name).to_string(),
                    replacement: format!("{} ", name),
                });
            }
        }

        // 第二阶段：PATH 缓存中前缀匹配且未被 builtin 覆盖的可执行文件
        for name in &self.path_executables {
            if name.starts_with(prefix) && !seen.contains(name.as_str()) {
                seen.insert(name.as_str());
                candidates.push(Pair {
                    display: name.clone(),
                    replacement: format!("{} ", name),
                });
            }
        }

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
