//! Tab 自动补全：仅在「首词」位置按前缀匹配 builtin 与 PATH 中的可执行文件。
//!
//! 设计要点：
//! - 候选源 1：`builtins::BUILTINS`（单一事实来源，新增 builtin 自动获得补全）。
//! - 候选源 2：`builtins::list_path_executables()`（启动期一次性扫描 PATH 并缓存到
//!   `ShellHelper.path_executables`，运行期不再重新扫描）。
//! - 触发条件：`line[..pos]` 不含空白；一旦进入参数区不做命令名补全，避免误补。
//! - 去重：同名时 **builtin 优先**，PATH 同名忽略；PATH 内部不同目录的同名也只取首个，
//!   与 `find_in_path` 按 PATH 顺序解析的执行行为对齐。用 `HashSet<&str>` 跟踪已加入名字。
//!
//! 双 TAB 状态机（多候选场景）：
//! - **0 候选**：no-op，line 不变。
//! - **1 候选**：直接替换为 `<name> `（末尾空格），与单候选 stage 行为一致。
//! - **≥2 候选 + 首次 TAB**：写 BEL（`\x07`）响铃，line 不变；通过 `last_tab_prefix`
//!   记住此次前缀。
//! - **≥2 候选 + 二次 TAB（前缀未变）**：换行后按字母序、双空格分隔列出全部候选；
//!   再换一行重画 `"$ <prefix>"`，让用户在原前缀上继续输入。
//! - **状态重置**：候选数变为 0/1，或前缀与上次记忆不同（用户在两次 TAB 间敲了字符）→
//!   清 `last_tab_prefix`，下次多候选重新进入"首次响铃"。
//!
//! 提示符常量同步：重画时使用的 `"$ "` 必须与 `main.rs::editor.readline("$ ")`
//! 字面一致；若修改提示符样式需同步两处。
//!
//! rustyline 协作语义：
//! - `Completer::complete` 签名为 `&self`，状态用 `Cell<Option<String>>` 内部可变性承载。
//! - 返回 `(pos, vec![])` 是干净 no-op：rustyline 不触碰 line buffer，亦不重绘提示符；
//!   我们通过直接 `print!` + `flush` 输出 BEL / 候选列表 / 重绘提示符。
//! - 二次 TAB 输出 `"\n<list>\n$ <prefix>"` 后，物理光标停在 `prefix` 末尾；rustyline
//!   认为 line=prefix、光标在末尾——位置一致，下次按键的增量 refresh 不会错位。
//! - rustyline 要求 helper 同时实现 Completer/Hinter/Highlighter/Validator + Helper（marker），
//!   非补全 trait 走默认实现即可。

use std::cell::Cell;
use std::collections::HashSet;
use std::io::{self, Write};

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result};

use crate::builtins::{list_path_executables, BUILTINS};

/// rustyline 的 Helper 聚合体；承载 builtin 列表（编译期常量）+ PATH 可执行文件缓存
/// + 双 TAB 状态机所需的「上次 TAB 时的前缀」。
///
/// `path_executables` 在 `new()` 中一次性填充，后续 TAB 触发只做内存遍历，
/// 牺牲对运行期 PATH 变化的感知换取更低补全延迟。
///
/// `last_tab_prefix` 用 `Cell<Option<String>>` 提供内部可变性（`Completer::complete`
/// 签名锁死 `&self`）；`String` 非 `Copy`，所以用 `take()/set()` 模式而非 `get()`。
pub struct ShellHelper {
    path_executables: Vec<String>,
    last_tab_prefix: Cell<Option<String>>,
}

impl ShellHelper {
    pub fn new() -> Self {
        ShellHelper {
            path_executables: list_path_executables(),
            last_tab_prefix: Cell::new(None),
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
            // 不重置 last_tab_prefix：参数区按 TAB 与命令名补全状态机无关。
            return Ok((pos, Vec::new()));
        }

        // 阶段 1：收集去重后的候选名（builtin 优先，PATH 内部及与 builtin 同名均跳过）
        let mut names: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for name in BUILTINS {
            if name.starts_with(prefix) {
                seen.insert(*name);
                names.push((*name).to_string());
            }
        }
        for name in &self.path_executables {
            if name.starts_with(prefix) && !seen.contains(name.as_str()) {
                seen.insert(name.as_str());
                names.push(name.clone());
            }
        }

        // 阶段 2：按候选数三态分支
        match names.len() {
            0 => {
                // 无候选：清状态机，let rustyline 维持 line 原样
                self.last_tab_prefix.set(None);
                Ok((pos, Vec::new()))
            }
            1 => {
                // 唯一候选：直接补全 + 末尾空格（沿用上 stage 语义）；清状态机
                self.last_tab_prefix.set(None);
                let name = names.into_iter().next().unwrap();
                let pair = Pair {
                    display: name.clone(),
                    replacement: format!("{} ", name),
                };
                Ok((0, vec![pair]))
            }
            _ => {
                // 多候选：按字母序排序后进入双 TAB 状态机
                names.sort();
                let prev = self.last_tab_prefix.take();
                let same_as_prev = prev.as_deref() == Some(prefix);
                if same_as_prev {
                    // 二次 TAB：列出 + 重画提示符；状态机已清空（take 取走）
                    let joined = names.join("  ");
                    // 物理输出：`\n<list>\n$ <prefix>`，光标停在 prefix 末尾
                    print!("\n{}\n$ {}", joined, prefix);
                    let _ = io::stdout().flush();
                } else {
                    // 首次 TAB（或前缀变化的新一轮）：响铃并记忆当前前缀
                    print!("\x07");
                    let _ = io::stdout().flush();
                    self.last_tab_prefix.set(Some(prefix.to_string()));
                }
                // 不让 rustyline 触碰 line buffer
                Ok((pos, Vec::new()))
            }
        }
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
