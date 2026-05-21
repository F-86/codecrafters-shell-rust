//! Tab 自动补全顶层：`ShellHelper` 结构体 + rustyline `Completer` 分发。
//!
//! 三套互斥状态机（命令名 / 参数路径 / 命令级脚本）通过物理子模块边界拆分。
//! 任一分支被触发即清掉对侧两个 Cell；提示符常量 `"$ "` 必须与
//! `main.rs::editor.readline("$ ")` 字面一致。
//!
//! 设计细节详见
//! [docs/DESIGN_DECISIONS.md#completion-state-machine](../../docs/DESIGN_DECISIONS.md#completion-state-machine)。
//!
//! ## 模块拆分
//!
//! - [`command`]  — 命令名 TAB 状态机（builtin + PATH executables 候选源）
//! - [`argpath`]  — 参数位置路径补全状态机（cwd 与嵌套目录）
//! - [`script`]   — `complete -C` 注册的命令级脚本补全状态机 + 上下文提取 + 子进程驱动
//! - [`helpers`]  — 三分支共享的纯函数（LCP / 路径分类 / 目录扫描等）

mod argpath;
mod command;
pub(crate) mod helpers;
mod script;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result};

use crate::builtins::list_path_executables;

/// rustyline 的 Helper 聚合体；承载 builtin 列表（编译期常量）+ PATH 可执行文件缓存
/// + 三套双 TAB 状态机所需的「上次 TAB 时的 key」。
///
/// `path_executables` 在 `new()` 中一次性填充，后续 TAB 触发只做内存遍历，
/// 牺牲对运行期 PATH 变化的感知换取更低补全延迟。
///
/// 三套 `last_tab_*` Cell 互相独立：用户在不同分支间切换不共享节奏；任一分支
/// 被触发都会清掉对侧两套（详见 docs/DESIGN_DECISIONS.md#completion-state-machine）。
pub struct ShellHelper {
    pub(super) path_executables: Vec<String>,
    pub(super) last_tab_prefix: Cell<Option<String>>,
    /// 参数位置双 TAB 状态机的「上次 TAB 时的 (dir_part, name_prefix) 对」。
    pub(super) last_tab_arg_key: Cell<Option<(String, String)>>,
    /// 命令级脚本分支双 TAB 状态机的「上次 TAB 时的 (cmd, current_word, prev_word) 三元组」。
    ///
    /// key 选三元组而非 `line[..pos]`：候选完全由脚本驱动，三元组捕获了「脚本 argv
    /// 输入快照」，跨「相同 argv 但 line 字面不同（多空白等）」也能正确合并节奏。
    /// `literal_len` 不入 key —— 它仅用于决定 replacement 起点，不影响候选集合。
    pub(super) last_tab_script_key: Cell<Option<(String, String, String)>>,
    /// `complete -C <path> <cmd>` 注册的补全脚本表的只读共享句柄。
    ///
    /// 与 `main.rs` 的 dispatch 写端共享同一份 `Rc`：写端在 `complete -C` 命令时
    /// `borrow_mut()` 写入；读端在本 helper 的 TAB 路径中 `borrow()` 查表。
    /// 单线程 REPL 串行节奏不会嵌套借用；查到 path 后立即 `cloned()` 出来，
    /// 缩短借用窗口至语句级，再 spawn 子进程，避免 Command::output() 阻塞期间
    /// 长期持有 RefCell 借用。
    pub(super) completions: Rc<RefCell<HashMap<String, String>>>,
}

impl ShellHelper {
    /// 构造 `ShellHelper`，启动期一次性扫描 PATH 缓存可执行文件名列表。
    ///
    /// `completions` 是 `complete -C` 注册表的共享句柄——dispatch 写端与本 helper
    /// 读端共用同一 `Rc`，单线程 REPL 无并发借用风险。
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::cell::RefCell;
    /// use std::collections::HashMap;
    /// use std::rc::Rc;
    /// use rustyline::Editor;
    ///
    /// let completions: Rc<RefCell<HashMap<String, String>>> =
    ///     Rc::new(RefCell::new(HashMap::new()));
    /// let helper = ShellHelper::new(completions.clone());
    /// let mut editor = Editor::new().unwrap();
    /// editor.set_helper(Some(helper));
    /// ```
    pub fn new(completions: Rc<RefCell<HashMap<String, String>>>) -> Self {
        ShellHelper {
            path_executables: list_path_executables(),
            last_tab_prefix: Cell::new(None),
            last_tab_arg_key: Cell::new(None),
            last_tab_script_key: Cell::new(None),
            completions,
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
        // 仅在首词位置触发命令名补全：光标左侧子串不含任何空白即视为仍在键入命令名。
        // 例：`ech` -> 命令名补全；`echo he` -> 文件名补全；`  ech` -> 文件名补全
        // （前导空白即进入参数区，与 bash 行为一致；空命令名场景由文件名分支自然 no-op）。
        let prefix = &line[..pos];
        if prefix.chars().any(|c| c.is_whitespace()) {
            // ---- 命令级补全分支：首词已结束的全部场景优先调注册脚本 ----
            if let Some(ctx) = script::extract_completer_context(prefix) {
                let registered: Option<String> =
                    self.completions.borrow().get(&ctx.cmd).cloned();
                let path = match registered {
                    Some(p) => p,
                    // registry 未命中：回退文件名补全（保留上 stage 行为）
                    None => return argpath::complete_filename_arg(self, line, pos),
                };
                return script::complete_with_script(self, line, pos, &path, &ctx);
            }
            // ctx 提取失败（tokenize 错）：回退既有文件名补全
            return argpath::complete_filename_arg(self, line, pos);
        }

        // 命令名分支被触发：参数分支与脚本分支的双 TAB 节奏作废，清掉对侧两套状态。
        self.last_tab_arg_key.set(None);
        self.last_tab_script_key.set(None);

        command::complete_command(self, prefix, pos)
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
