//! PATH 可执行文件查找：`find_in_path`（单文件命中） + `list_path_executables`（启动期扫描）。
//!
//! 这是 `run_type` 命中判定、`exec::run_external` PATH 解析、`completion`
//! TAB 候选源三方共享的唯一数据源——避免可执行性判定逻辑分叉。

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// 按 PATH 顺序查找可执行文件。
/// 命中条件：文件存在、是普通文件、Unix 执行位（owner/group/other 任一）置位。
/// 目录不存在 / 无权限读取 / 非普通文件等场景静默跳过，与 bash 实际行为一致。
///
/// # Examples
///
/// ```ignore
/// // 典型用例：dispatch 阶段判断命令名是 builtin 还是 PATH 命中
/// if let Some(path) = find_in_path("ls") {
///     // `path` 是绝对路径，如 PathBuf::from("/usr/bin/ls")
///     println!("ls is {}", path.display());
/// } else {
///     eprintln!("ls: command not found");
/// }
/// ```
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        let Ok(meta) = candidate.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if meta.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

/// 启动期一次性扫描 PATH，返回所有可执行文件 basename 的有序列表。
///
/// 与 `find_in_path` 共享同一可执行性判定标准（`is_file()` + `0o111` 任一执行位），
/// 作为 TAB 补全候选源使用。返回顺序：按 PATH 顺序、目录内 `read_dir` 顺序——
/// **不去重**，去重责任交由调用方（如 completion 端按 builtin 优先策略合并）。
///
/// 错误处理（与 bash 行为一致）：
/// - `PATH` 环境变量缺失 → 直接返回空 vec
/// - 某个 PATH 目录不存在 / 无权限 / 非目录 → `read_dir` 失败，静默跳过该目录
/// - 单个 entry 的 `metadata()` 读取失败 → 静默跳过该 entry
///
/// 不向 stderr 写任何错误信息：TAB 补全是高频热路径，避免污染交互终端。
pub fn list_path_executables() -> Vec<String> {
    let mut out = Vec::new();
    let Some(path_var) = std::env::var_os("PATH") else {
        return out;
    };
    for dir in std::env::split_paths(&path_var) {
        // read_dir 失败（目录不存在 / 无权限 / 非目录）静默跳过整目录
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            // metadata 读取失败的 entry（如 symlink 悬空）静默跳过
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if meta.permissions().mode() & 0o111 == 0 {
                continue;
            }
            // file_name 在 Linux 上一般是合法 UTF-8；遇到非 UTF-8 字节走 lossy
            // 转换并入候选——后续作为命令名补全到 line 时仍是合法 String。
            out.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    out
}
