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
//! 参数位置单匹配的目录识别：
//! - 候选若为目录 → 替换为 `<full>/`，**不**加尾空格，便于继续 TAB 进入下一层。
//! - 候选若为文件 → 替换为 `<full> `（保持上 stage 语义）。
//! - 跟随 symlink（与 bash/zsh/fish 一致）；stat 失败安全退化为文件分支。
//!
//! 多候选状态机（四态）：
//! - **0 候选**：no-op，line 不变。
//! - **1 候选**：直接替换为 `<name> `（末尾空格），与单候选 stage 行为一致。
//! - **≥2 候选 + LCP 可扩展（`lcp.len() > prefix.len()`）**：把 `line[0..pos]` 替换为
//!   最长公共前缀 LCP，光标停 LCP 末尾，**不加尾空格**；通过单 Pair 让 rustyline
//!   走与「1 候选」同一替换路径，零额外打印。
//! - **≥2 候选 + LCP 不可扩展（`lcp == prefix`）**：进入双 TAB 路径——
//!     - 首次 TAB：写 BEL（`\x07`）响铃，line 不变，`last_tab_prefix` 记住此次前缀。
//!     - 二次 TAB（前缀未变）：换行后按字母序、双空格分隔列出全部候选；再换一行重画
//!       `"$ <prefix>"`，让用户在原前缀上继续输入。
//! - **状态重置**：候选数变为 0/1、LCP 扩展成功、或前缀与上次记忆不同（用户在两次
//!   TAB 间敲了字符）→ 清 `last_tab_prefix`，下次多候选重新进入"首次响铃"。
//!
//! LCP 算法：候选已字典序排序后，**首末两项的公共前缀 == 全集 LCP**（介于首末之间
//! 的串其各位置字符必落于首末项相应位置之间或相等，故全集 LCP 与首末项 LCP 相等）。
//! O(n + L) 优于朴素 O(n·L)。
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
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result};

use crate::builtins::{list_path_executables, BUILTINS};
use crate::parser::tokenize;

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
    /// 参数位置双 TAB 状态机的「上次 TAB 时的 (dir_part, name_prefix) 对」。
    ///
    /// 与命令名分支的 `last_tab_prefix` 独立：两者 key 类型不同（命令名是单 String =
    /// 整行 line[..pos]；参数是切分后的二元组），语义独立——用户在命令名按 TAB
    /// 与在参数按 TAB 互不影响节奏。任一分支返回路径都会清掉对侧字段，
    /// 避免「先在命令名 BEL 一次→切到参数立即列出」之类的污染。
    last_tab_arg_key: Cell<Option<(String, String)>>,
}

impl ShellHelper {
    pub fn new() -> Self {
        ShellHelper {
            path_executables: list_path_executables(),
            last_tab_prefix: Cell::new(None),
            last_tab_arg_key: Cell::new(None),
        }
    }

    /// 参数位置文件名补全（本 stage 仅处理 cwd、单匹配场景）。
    ///
    /// 调用前置条件：`line[..pos]` 至少含一个空白（已离开命令名区）；该判定在
    /// `Completer::complete` 入口完成，本方法不再重复。
    ///
    /// 行为：
    /// - prefix 提取失败（tokenize 错误等）→ 静默 no-op，不响铃（避免未闭合引号噪音）。
    /// - 末尾空白 → `prefix = ""`：等价于"列出 cwd 全部 entry"，由后续候选数分支统一处理。
    /// - 字面对齐校验失败（line 字面尾段 != prefix；引号被剥离会触发）→ no-op，
    ///   留给后续 stage 实现引号场景。
    /// - 候选数 0 或 ≥2 → 显式向 stdout 写 BEL（`\x07`）+ flush，line 不变；
    ///   rustyline `List` 模式在空候选时还会自动 beep 一次作为兜底，两次 BEL
    ///   在终端语义上等价单次响铃，tester 校验「出现 \x07」不卡数量。
    /// - 候选数 = 1 → 把 `[pos - prefix.len(), pos)` 区间替换为 `<full> ` 或 `<full>/`。
    ///
    /// 双 TAB 状态机由 `self.last_tab_arg_key` 独立承载（与命令名分支 `last_tab_prefix`
    /// 互不干扰）；任何返回路径都会清掉 `last_tab_prefix`，避免命令名节奏污染参数节奏。
    fn complete_filename_arg(
        &self,
        line: &str,
        pos: usize,
    ) -> Result<(usize, Vec<Pair>)> {
        let line_to_pos = &line[..pos];

        // 1. 提取前缀；tokenize 失败 → 静默 no-op。空 prefix（末尾空白）是合法输入：
        //    语义为"列出 cwd 全部 entry"，与 `dir/<TAB>` 列 `dir/` 全部 entry 同构。
        let prefix = match extract_arg_prefix(line_to_pos) {
            Some(p) => p,
            None => {
                // tokenize 失败仍清掉双 TAB 状态机（已离开任何有意义的补全节奏）
                self.last_tab_arg_key.set(None);
                self.last_tab_prefix.set(None);
                return Ok((pos, Vec::new()));
            }
        };

        // 2. 字面对齐校验：本 stage 测试不含引号/转义，故 prefix 与 line 末尾
        //    字面应一致；不一致说明 tokenize 做了剥离（如 `cat 're<TAB>`），
        //    这种情况下 pos - prefix.len() 起点会错位，按 no-op 退避。
        if prefix.len() > pos {
            self.last_tab_arg_key.set(None);
            self.last_tab_prefix.set(None);
            return Ok((pos, Vec::new()));
        }
        let start = pos - prefix.len();
        if &line[start..pos] != prefix.as_str() {
            self.last_tab_arg_key.set(None);
            self.last_tab_prefix.set(None);
            return Ok((pos, Vec::new()));
        }

        // 3. 按最后一个 '/' 切分目录与叶子前缀；不含 '/' 时 dir_part = ""，等价 cwd。
        let (dir_part, name_prefix) = split_dir_and_name(&prefix);
        let scan_dir: &Path = if dir_part.is_empty() {
            Path::new(".")
        } else {
            Path::new(dir_part)
        };
        let mut candidates = match_files_in_dir(scan_dir, name_prefix);

        // 命令名分支状态独立但语义上互斥：任一分支被触发都视为「另一边的节奏已断」
        self.last_tab_prefix.set(None);

        // 4. 候选数分支
        match candidates.len() {
            0 => {
                // 0 候选：BEL（保留上 stage 语义）；清状态，line 不变。
                self.last_tab_arg_key.set(None);
                print!("\x07");
                let _ = io::stdout().flush();
                Ok((pos, Vec::new()))
            }
            1 => {
                self.last_tab_arg_key.set(None);
                let entry = candidates.into_iter().next().unwrap();
                // 拼回 dir_part：dir_part 含尾 '/' 或为空字符串，直接拼接得到完整 token。
                let full = format!("{}{}", dir_part, entry);
                // 目录 → 尾 `/`、不加空格；文件 → 尾空格。stat 失败按文件退化（安全方向）。
                let kind = classify_path(Path::new(&full));
                Ok((start, vec![format_arg_completion(&full, kind)]))
            }
            _ => {
                // ≥2 候选：先字母序排序（match_files_in_dir 不保证 read_dir 顺序）
                candidates.sort();

                // 4a. LCP 扩展（与命令名分支对称）：若候选叶子名的最长公共前缀
                //     长于当前 name_prefix，则把 line[start..pos] 替换为
                //     `dir_part + lcp`（不带尾空格 / `/`），让用户继续打字以收敛候选。
                //     首末项 LCP == 全集 LCP（已排序前提）。
                let lcp = longest_common_prefix(&candidates[0], candidates.last().unwrap());
                if lcp.len() > name_prefix.len() {
                    self.last_tab_arg_key.set(None);
                    let replacement = format!("{}{}", dir_part, lcp);
                    let pair = Pair {
                        display: replacement.clone(),
                        replacement,
                    };
                    return Ok((start, vec![pair]));
                }

                // 4b. LCP 不可扩展：进入双 TAB 状态机。
                //     状态 key 用 (dir_part, name_prefix) 对：跨命令名节奏复用，
                //     用户在两次 TAB 之间改了命令名（但 token 切分结果相同）仍算同一轮。
                let current_key = (dir_part.to_string(), name_prefix.to_string());
                let prev = self.last_tab_arg_key.take();
                let same_as_prev = prev.as_ref() == Some(&current_key);
                if same_as_prev {
                    // 二次 TAB：列出 + 重画提示符。状态机已由 take() 清空。
                    // 列出时每候选 stat 一次以判类型（目录拼尾 '/'）；本 stage 候选数
                    // 在 tester 场景下通常为 2~3 个，stat 开销可忽略。
                    let listed: Vec<String> = candidates
                        .iter()
                        .map(|name| {
                            let full = format!("{}{}", dir_part, name);
                            match classify_path(Path::new(&full)) {
                                MatchKind::Directory => format!("{}/", name),
                                MatchKind::File => name.clone(),
                            }
                        })
                        .collect();
                    let joined = listed.join("  ");
                    // 物理输出：`\n<list>\n$ <line[..pos]>`，光标停在 line 末尾。
                    // 注意：必须重画整段 line[..pos]（含命令名 + 已输入的参数部分），
                    // 不能只重画 prefix——否则用户看到的会是 `$ bar` 而非 `$ stat bar`。
                    print!("\n{}\n$ {}", joined, line_to_pos);
                    let _ = io::stdout().flush();
                } else {
                    // 首次 TAB（或 key 变化的新一轮）：BEL + 记忆当前 key
                    print!("\x07");
                    let _ = io::stdout().flush();
                    self.last_tab_arg_key.set(Some(current_key));
                }
                // 不让 rustyline 触碰 line buffer
                Ok((pos, Vec::new()))
            }
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
            // 进入参数补全分支：由 complete_filename_arg 内部负责清空 last_tab_prefix。
            return self.complete_filename_arg(line, pos);
        }

        // 命令名分支被触发：参数分支的双 TAB 节奏作废，清掉对侧状态。
        self.last_tab_arg_key.set(None);

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
                // 多候选：先字母序排序，再判断 LCP 是否可扩展
                names.sort();
                // 首末项的 LCP 即全集 LCP（已排序前提下）
                let last = names.last().unwrap();
                let lcp = longest_common_prefix(&names[0], last);
                if lcp.len() > prefix.len() {
                    // LCP 扩展：清状态 + 让 rustyline 把 line[0..pos] 替换为 lcp（不带尾空格）
                    self.last_tab_prefix.set(None);
                    let s = lcp.to_string();
                    let pair = Pair {
                        display: s.clone(),
                        replacement: s,
                    };
                    return Ok((0, vec![pair]));
                }
                // LCP == prefix：无法扩展，进入双 TAB 状态机
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

/// 返回 `a` 与 `b` 的最长公共前缀切片（按 UTF-8 char 边界安全截取）。
///
/// 实现说明：用 `char_indices` 同步遍历两串，遇到首个不一致字符即按其字节起点切片；
/// 若一串先耗尽则较短串自身即为 LCP。返回值借自 `a`，生命周期与 `a` 一致。
fn longest_common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let mut ai = a.char_indices();
    let mut bi = b.char_indices();
    loop {
        match (ai.next(), bi.next()) {
            (Some((i, ca)), Some((_, cb))) if ca == cb => {
                // 继续比较；记住下一个 char 的起点用作潜在切点
                let _ = i;
            }
            (Some((i, _)), Some(_)) => return &a[..i], // 首个不一致字符在 a 的字节位置 i
            (None, _) => return a,                     // a 已全部覆盖
            (Some((i, _)), None) => return &a[..i],    // b 已耗尽，截到 a 的当前位置
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
/// - `None`：tokenize 失败（未闭合引号、行尾孤立反斜杠等）；调用方按静默 no-op 处理，
///   不响铃以免对未闭合引号场景产生噪音。
///
/// 注：本 stage 测试不含引号/转义，所以非空 prefix 与 line 中字面子串等长；调用方
/// 仍需做一道字面对齐校验以兜底未覆盖的情形（见 `complete_filename_arg`）。
fn extract_arg_prefix(line_to_pos: &str) -> Option<String> {
    // 末尾空白 → 空 prefix（"列出全部"语义），不再走 tokenize 取最后 token
    if line_to_pos.chars().next_back().map_or(true, |c| c.is_whitespace()) {
        return Some(String::new());
    }
    let tokens = tokenize(line_to_pos).ok()?;
    tokens.into_iter().last()
}

/// 把参数 token 切分为 (dir_part, name_prefix)，供嵌套路径补全使用。
///
/// 切分规则：以 token 中**最后一个 `/`** 为切点；`dir_part` 始终包含尾 `/`。
/// 不含 `/` 时退化为 `("", token)`，让调用方按 cwd 场景处理（与上 stage 行为字面等价）。
///
/// 实现说明：`'/'` 是 ASCII 字节，`rfind('/')` 直接定位字节位置，`split_at` 在
/// UTF-8 串上按字节切片不会破坏 char 边界。返回 `&str` 切片避免分配。
///
/// 例：
/// - `"f"`         → `("", "f")`
/// - `"path/to/f"` → `("path/to/", "f")`
/// - `"path/to/"`  → `("path/to/", "")`
/// - `"/etc/h"`    → `("/etc/", "h")`
/// - `""`          → `("", "")`
fn split_dir_and_name(token: &str) -> (&str, &str) {
    match token.rfind('/') {
        Some(idx) => token.split_at(idx + 1),
        None => ("", token),
    }
}

/// 扫描指定目录，返回所有以 `name_prefix` 字面开头的 entry 叶子名（**不含 dir 前缀**）。
///
/// - `dir` 由 OS 解析：相对路径相对 cwd，绝对路径直接定位（如 `/etc/`）。
/// - 不区分 file/dir（本 stage 测试只放普通文件；目录尾随 `/` 留待后续 stage）。
/// - 隐藏文件天然纳入：`read_dir` 不会自动过滤 `.` 开头条目，且本函数不主动跳过。
/// - I/O 失败（不存在 / 非目录 / 权限 / DirEntry 解析失败）静默返回空 Vec：
///   补全是交互路径，写错误日志会污染用户输入区。
/// - 调用方负责拼回 dir 前缀形成完整路径。
/// - 复杂度 O(N)，N 为目标目录条目数；TAB 是低频交互，不做缓存。
fn match_files_in_dir(dir: &Path, name_prefix: &str) -> Vec<String> {
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
/// 比 `format_arg_completion(&full, true)` 自解释；未来扩展（如 `Symlink` /
/// `Executable` / `Hidden`）时函数签名不变，符合 Rust API guidelines 关于
/// "prefer enums to bool flags" 的建议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    File,
    Directory,
}

/// 判定 `path` 是文件还是目录（跟随 symlink，与 bash/zsh/fish 一致）。
///
/// `fs::metadata` 与 `symlink_metadata` 的关键差异：前者跟随 symlink 取最终目标的
/// 元数据，后者返回 symlink 自身。真实 shell 的目录补全按目标类型决定尾随字符
/// （指向目录的 symlink 也加 `/`），故选用 `fs::metadata`。
///
/// 错误处理：任何 I/O 失败（路径不存在 / 权限不足 / TOCTOU 即读即删）一律退化为
/// `MatchKind::File` —— 对调用方而言，"加尾空格" 是语义安全的退化（用户至多在
/// 不存在路径上多打个空格），优于"目录被识别为文件后无法继续 TAB" 的反向 bug。
fn classify_path(path: &Path) -> MatchKind {
    match fs::metadata(path) {
        Ok(m) if m.is_dir() => MatchKind::Directory,
        _ => MatchKind::File,
    }
}

/// 把单匹配的完整路径与类型格式化为 rustyline 的 `Pair`。
///
/// - `MatchKind::Directory` → `replacement = "{full}/"`，**无**尾空格，便于用户
///   立即再次按 TAB 进入下一层。
/// - `MatchKind::File`      → `replacement = "{full} "`，加尾空格，沿用上 stage
///   文件名补全语义。
///
/// `display` 与 `replacement` 视觉一致（目录的 `display` 也带 `/`）：rustyline 在
/// 多候选列表场景下会读取 `display`，与 bash `ls -F` / `complete` 的展示风格对齐，
/// 便于后续多匹配 stage 直接复用。
fn format_arg_completion(full: &str, kind: MatchKind) -> Pair {
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
    use super::longest_common_prefix as lcp;
    use super::extract_arg_prefix;

    #[test]
    fn lcp_basic() {
        assert_eq!(lcp("xyz_foo", "xyz_foo_bar"), "xyz_foo");
        assert_eq!(lcp("xyz_foo_bar", "xyz_foo_bar_baz"), "xyz_foo_bar");
        assert_eq!(lcp("xyz_bar", "xyz_quz"), "xyz_");
        assert_eq!(lcp("abc", "xyz"), "");
        assert_eq!(lcp("", "abc"), "");
        assert_eq!(lcp("abc", ""), "");
        assert_eq!(lcp("same", "same"), "same");
    }

    #[test]
    fn extract_prefix_normal() {
        // 普通参数：取最后一个 token
        assert_eq!(extract_arg_prefix("cat re"), Some("re".to_string()));
        assert_eq!(extract_arg_prefix("xyz read"), Some("read".to_string()));
        // 多个参数：仍取最后一个
        assert_eq!(
            extract_arg_prefix("cat foo bar bz"),
            Some("bz".to_string())
        );
    }

    #[test]
    fn extract_prefix_trailing_space_returns_empty() {
        // 末尾空白 → 空 prefix（"列出全部"语义），由调用方按候选数分支统一处理
        assert_eq!(extract_arg_prefix("cat re "), Some(String::new()));
        assert_eq!(extract_arg_prefix("cat "), Some(String::new()));
        // 多空白同样返回空 prefix
        assert_eq!(extract_arg_prefix("cat   "), Some(String::new()));
    }

    #[test]
    fn extract_prefix_tokenize_error_returns_none() {
        // 未闭合单引号 → tokenize 报错 → no-op
        assert_eq!(extract_arg_prefix("cat 'unclosed"), None);
        // 未闭合双引号
        assert_eq!(extract_arg_prefix("cat \"unclosed"), None);
        // 行尾孤立反斜杠
        assert_eq!(extract_arg_prefix("cat foo\\"), None);
    }

    // 以下集成性测试依赖 cargo 测试工作目录 = crate root 这一既定行为。
    // 我们使用项目自带的 `Cargo.toml` / `Cargo.lock` / `README.md` 作为只读 fixture。
    #[test]
    fn match_files_finds_unique_prefix() {
        // `Cargo.t` 在本仓库内仅匹配 Cargo.toml
        let v = super::match_files_in_dir(std::path::Path::new("."), "Cargo.t");
        assert_eq!(v, vec!["Cargo.toml".to_string()]);
    }

    #[test]
    fn match_files_multi_match_returns_all() {
        // `Cargo.` 匹配 Cargo.toml 与 Cargo.lock
        let mut v = super::match_files_in_dir(std::path::Path::new("."), "Cargo.");
        v.sort();
        assert_eq!(
            v,
            vec!["Cargo.lock".to_string(), "Cargo.toml".to_string()]
        );
    }

    #[test]
    fn match_files_no_match_empty() {
        let v = super::match_files_in_dir(std::path::Path::new("."), "zzz_no_such_prefix_");
        assert!(v.is_empty());
    }

    // ---- split_dir_and_name 边界用例 ----
    #[test]
    fn split_no_slash() {
        assert_eq!(super::split_dir_and_name("f"), ("", "f"));
    }

    #[test]
    fn split_empty_token() {
        assert_eq!(super::split_dir_and_name(""), ("", ""));
    }

    #[test]
    fn split_relative_path() {
        assert_eq!(super::split_dir_and_name("path/to/f"), ("path/to/", "f"));
    }

    #[test]
    fn split_trailing_slash() {
        // 末尾即最后 '/'：name_prefix 为空，dir_part 含尾 '/'
        assert_eq!(super::split_dir_and_name("path/to/"), ("path/to/", ""));
    }

    #[test]
    fn split_absolute_path() {
        assert_eq!(super::split_dir_and_name("/etc/h"), ("/etc/", "h"));
    }

    #[test]
    fn split_multi_level() {
        assert_eq!(super::split_dir_and_name("a/b/c/d"), ("a/b/c/", "d"));
    }

    // ---- match_files_in_dir 嵌套目录用例 ----
    #[test]
    fn match_files_nested_finds_entry() {
        // src/ 下确有 completion.rs；前缀 "comp" 应至少命中它
        let v = super::match_files_in_dir(std::path::Path::new("src"), "comp");
        assert!(
            v.iter().any(|n| n == "completion.rs"),
            "expected completion.rs in {:?}",
            v
        );
    }

    #[test]
    fn match_files_nonexistent_dir_returns_empty() {
        let v = super::match_files_in_dir(
            std::path::Path::new("zzz_no_such_dir_xyz_qqq"),
            "",
        );
        assert!(v.is_empty());
    }

    // ---- classify_path 用例 ----
    // 依赖 cargo 测试 cwd = crate root：src/ 是目录，Cargo.toml 是文件。
    #[test]
    fn classify_path_directory() {
        assert_eq!(
            super::classify_path(std::path::Path::new("src")),
            super::MatchKind::Directory
        );
    }

    #[test]
    fn classify_path_file() {
        assert_eq!(
            super::classify_path(std::path::Path::new("Cargo.toml")),
            super::MatchKind::File
        );
    }

    #[test]
    fn classify_path_missing_falls_back_to_file() {
        // 路径不存在 → 退化为 File（加尾空格是安全方向）
        assert_eq!(
            super::classify_path(std::path::Path::new("zzz_no_such_path_qqq")),
            super::MatchKind::File
        );
    }

    // ---- format_arg_completion 用例 ----
    #[test]
    fn format_arg_completion_file_flat() {
        let p = super::format_arg_completion("foo.txt", super::MatchKind::File);
        assert_eq!(p.display, "foo.txt");
        assert_eq!(p.replacement, "foo.txt ");
    }

    #[test]
    fn format_arg_completion_directory_flat() {
        let p = super::format_arg_completion("project", super::MatchKind::Directory);
        assert_eq!(p.display, "project/");
        assert_eq!(p.replacement, "project/");
    }

    #[test]
    fn format_arg_completion_file_nested() {
        let p = super::format_arg_completion("path/to/foo.txt", super::MatchKind::File);
        assert_eq!(p.display, "path/to/foo.txt");
        assert_eq!(p.replacement, "path/to/foo.txt ");
    }

    #[test]
    fn format_arg_completion_directory_nested() {
        let p = super::format_arg_completion("pig/dog", super::MatchKind::Directory);
        assert_eq!(p.display, "pig/dog/");
        assert_eq!(p.replacement, "pig/dog/");
    }
}
