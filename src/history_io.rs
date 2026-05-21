//! `$HISTFILE` 启动加载 / 退出保存 + `history -r/-w/-a` 三段文件 IO。
//!
//! 与 `builtins::history` 的渲染逻辑职责分离：本模块只负责 rustyline `Editor`
//! 与文件系统之间的双向同步；`run_history` 只负责 `&[String]` 的渲染格式。
//!
//! 静默失败策略集中在本模块（所有路径不向 stderr 写错误、不阻断 REPL），与 bash
//! 真实行为一致：`HISTFILE` 未设置 / 文件不存在 / 权限不足等场景均视为 no-op。
//!
//! 详见 docs/DESIGN_DECISIONS.md#background-reaping（reaping 章节末尾「静默失败」原则
//! 同样适用于本模块）。

use std::io::{BufRead, BufReader, BufWriter, Write};

use rustyline::history::{History, SearchDirection};
use rustyline::Editor;

use crate::completion::ShellHelper;

/// rustyline 14 中 `Editor` 默认使用 `FileHistory` 作为 history 后端；
/// 用 type alias 把 5 处签名收敛到一行，main.rs 不再写裸 `Editor<ShellHelper, FileHistory>`。
pub type ShellEditor = Editor<ShellHelper, rustyline::history::FileHistory>;

/// 启动时：若设置了 `$HISTFILE` 且文件可读，按行加载历史条目入 rustyline editor。
///
/// 行为：等价于在 main 入口对 `$HISTFILE` 做一次隐式 `history -r $HISTFILE`：
/// - `HISTFILE` 未设置 / 含非 UTF-8 字节 → 静默跳过
/// - `HISTFILE=""` → 静默跳过（不打开文件）
/// - 文件不存在 / 无权限 / 行 IO 错误 → 静默忽略，不阻断启动
/// - 空行跳过，避免污染 history 编号
pub fn load_history_from_envfile(editor: &mut ShellEditor) {
    let Ok(path) = std::env::var("HISTFILE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let Ok(file) = std::fs::File::open(&path) else {
        return;
    };
    let reader = BufReader::new(file);
    for line in reader.lines().flatten() {
        if !line.is_empty() {
            let _ = editor.add_history_entry(line);
        }
    }
}

/// 退出时：把 editor 内存历史按时序全量覆写至 `$HISTFILE`。
///
/// 等价于在 shell 退出前做一次隐式 `history -w $HISTFILE`：
/// - `File::create`（O_WRONLY|O_CREAT|O_TRUNC）+ `BufWriter` + `writeln`
/// - 任意失败静默忽略，不写 stderr、不阻断退出
/// - history 为空 → 写出空文件（与 bash `history -w` 空历史一致）
///
/// 调用点：`exit` builtin arm + 主循环后 Ctrl-D 路径。两处必须共用本 helper 保持
/// 行为精确一致（否则 exit 与 Ctrl-D 行为分裂）。
pub fn save_history_to_envfile(editor: &ShellEditor) {
    let Ok(path) = std::env::var("HISTFILE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let Ok(file) = std::fs::File::create(&path) else {
        return;
    };
    let h = editor.history();
    let mut w = BufWriter::new(file);
    for i in 0..h.len() {
        if let Ok(Some(sr)) = h.get(i, SearchDirection::Forward) {
            let _ = writeln!(w, "{}", sr.entry);
        }
    }
    let _ = w.flush();
}

/// `history -r <path>`：从文件按行追加历史到 rustyline editor 内部 history 栈。
///
/// 边界处理（与 -w / -a 静默风格对称）：
/// - 文件不存在 / 无权限 / 单行 IO 错误：静默忽略，不写 stderr、不阻断 REPL
/// - 空行：`is_empty()` 跳过，不污染历史编号
pub fn run_history_read(editor: &mut ShellEditor, path: &str) {
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let reader = BufReader::new(file);
    for line in reader.lines().flatten() {
        if !line.is_empty() {
            let _ = editor.add_history_entry(line);
        }
    }
}

/// `history -w <path>`：把内存中的全部历史条目按时序覆盖写入文件，末尾保留尾换行。
///
/// 关键技术点：
/// - `File::create`（= O_WRONLY|O_CREAT|O_TRUNC）实现「不存在则创建、存在则覆盖」语义
/// - `writeln!` 自动加 `\n`（最后一行也带尾换行，与 bash 行为一致）
/// - `BufWriter` + 显式 `flush()` 让错误路径明确（即便后续仍走静默忽略策略）
///
/// 失败（文件创建 / 写入 / flush 错误）静默忽略，不写 stderr、不阻断 REPL。
pub fn run_history_write(editor: &ShellEditor, path: &str) {
    let h = editor.history();
    let mut entries: Vec<String> = Vec::with_capacity(h.len());
    for i in 0..h.len() {
        if let Ok(Some(sr)) = h.get(i, SearchDirection::Forward) {
            entries.push(sr.entry.into_owned());
        }
    }
    if let Ok(file) = std::fs::File::create(path) {
        let mut w = BufWriter::new(file);
        for entry in &entries {
            let _ = writeln!(w, "{}", entry);
        }
        let _ = w.flush();
    }
}

/// `history -a <path>`：把「自上次 -a 之后」内存中新增的历史条目按时序**追加**写入。
///
/// 增量追加语义（bash Notes 第 2 条：only append since last -a）：
/// - `start = last_appended_len.min(total)` 防御性裁剪
/// - 仅写出 `[start, total)` 切片
/// - 文件**成功打开**后即推进 `*last_appended_len = total`；写入 / flush 失败不回滚
///   （与 bash 一致：失败的 -a 不重试同一批，避免重复写）
/// - 首次 -a（last_appended_len=0）：写出当前内存全部历史
/// - 文件打开失败：游标不推进，下次 -a 仍尝试本批（数据不丢失）
///
/// 关键技术点：
/// - `OpenOptions::new().create(true).append(true)` = O_WRONLY|O_CREAT|O_APPEND
/// - `.min(total)` 防御：rustyline 14 内部 ignore_dups 等机制可能导致 len() 收缩
pub fn run_history_append(editor: &ShellEditor, path: &str, last_appended_len: &mut usize) {
    let h = editor.history();
    let total = h.len();
    let start = (*last_appended_len).min(total);
    let mut entries: Vec<String> = Vec::with_capacity(total - start);
    for i in start..total {
        if let Ok(Some(sr)) = h.get(i, SearchDirection::Forward) {
            entries.push(sr.entry.into_owned());
        }
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let mut w = BufWriter::new(file);
        for entry in &entries {
            let _ = writeln!(w, "{}", entry);
        }
        let _ = w.flush();
        // 文件成功打开即推进游标
        *last_appended_len = total;
    }
}

/// 从 rustyline editor 收集本会话所有 history 条目为 `Vec<String>`。
///
/// 由 `history` builtin dispatch arm 调用，把收集到的切片传给 `run_history`
/// 渲染。抽出为独立函数是为了让 main.rs 不直接依赖 `rustyline::history::History`
/// trait + `SearchDirection`。
pub fn collect_history_entries(editor: &ShellEditor) -> Vec<String> {
    let h = editor.history();
    let mut entries: Vec<String> = Vec::with_capacity(h.len());
    for i in 0..h.len() {
        if let Ok(Some(sr)) = h.get(i, SearchDirection::Forward) {
            entries.push(sr.entry.into_owned());
        }
    }
    entries
}
