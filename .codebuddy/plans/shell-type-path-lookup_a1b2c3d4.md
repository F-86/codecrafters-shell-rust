---
name: shell-type-path-lookup
overview: "扩展 `type` 内建：当目标不是 shell builtin 时，按 PATH 环境变量顺序搜索可执行文件，命中输出 `<cmd> is <full_path>`，全部未命中输出 `<cmd>: not found`。"
todos:
  - id: impl-find-in-path
    content: "新增辅助函数 find_in_path(name)：使用 env::var_os(\"PATH\") + env::split_paths 遍历，命中（is_file 且具备 Unix 执行权限 mode & 0o111 != 0）即返回完整 PathBuf；目录不存在/权限缺失静默跳过"
    status: pending
  - id: extend-type-branch
    content: "改造 src/main.rs 的 \"type\" 分支：先判 BUILTINS 命中输出 builtin 提示；否则调用 find_in_path，命中输出 `<target> is <path>`，未命中输出 `<target>: not found`"
    status: pending
    dependencies:
      - impl-find-in-path
  - id: verify-path-lookup
    content: "本地验证：type ls / type grep 输出真实路径；type echo/exit/type 仍为 builtin 提示；type invalid_command 输出 not found；PATH 含不存在目录时无报错；既有 echo/exit/未知命令/空行/EOF 无回归"
    status: pending
    dependencies:
      - extend-type-branch
---

## 产品概述

在现有 Rust shell（REPL + `echo`/`exit`/`type` 内建 + 未知命令报错）基础上，扩展 `type` 内建的查询能力：当查询目标不是 shell builtin 时，按 `PATH` 环境变量列出的目录顺序查找可执行文件。这是后续阶段"执行外部程序"的前置基础（PATH 解析与可执行文件定位逻辑可复用）。

## 核心功能

- `type` 查询顺序变更为：builtin 优先 → PATH 搜索 → 都未命中输出 `not found`
- PATH 搜索规则：
  - 使用 OS 无关的 `env::split_paths` 拆分（自动处理 `:`/`;`）
  - 顺序遍历每个目录，取首个"存在 + 是文件 + 具备执行权限"的命中
  - 不存在或无权限读取的目录静默跳过
- 输出格式：
  - 命中可执行文件 → `<target> is <full_path>`
  - 未命中 → `<target>: not found`

## 验收要点

- `type ls` → `ls is /usr/bin/ls`（或当前 PATH 下首个命中的实际路径）
- `type grep` → `grep is /usr/bin/grep`
- `type echo` → `echo is a shell builtin`（builtin 优先级最高）
- `type exit` → `exit is a shell builtin`
- `type type` → `type is a shell builtin`
- `type invalid_command` → `invalid_command: not found`
- PATH 中含不存在目录（如 `/nonexistent`）不会触发 panic 或错误输出
- 既有阶段无回归：`echo`、`exit`、未知命令的 `command not found`、空行跳过、EOF 自然退出

## 技术栈

- 语言：Rust（edition = "2024", rust-version = "1.95"）
- 仅使用标准库；不引入新依赖
- 运行环境：Linux（CodeCrafters tester 与本项目均 POSIX）；权限检查使用 `std::os::unix::fs::PermissionsExt`

## 实现思路

1. 在 `src/main.rs` 顶部追加 `use`：
   - `std::env`（取 PATH、拆分路径）
   - `std::path::PathBuf`（返回值类型）
   - `std::os::unix::fs::PermissionsExt`（Unix 权限位）

2. 新增辅助函数：

```rust
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        let Ok(meta) = candidate.metadata() else { continue };
        if !meta.is_file() { continue; }
        if meta.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}
```

3. 改造 `"type"` 分支：

```rust
"type" => {
    if let Some(target) = parts.next() {
        if BUILTINS.contains(&target) {
            println!("{} is a shell builtin", target);
        } else if let Some(path) = find_in_path(target) {
            println!("{} is {}", target, path.display());
        } else {
            println!("{}: not found", target);
        }
    }
}
```

4. 其他分支与控制流保持不变，blast radius 仅辅助函数 + 一处分支增删。

## 关键技术决策

- **`env::split_paths` 而非手动 `split(':')`**：题目明确建议使用 OS 无关的分隔符处理；标准库 API 在 Windows 上会用 `;`，在 Unix 上用 `:`，避免移植性陷阱。
- **`is_file()` 而非 `exists()`**：避免目录意外被当作可执行（PATH 内的同名子目录是真实存在的边界）。
- **Unix 权限位 `& 0o111`**：检查 owner/group/other 任一执行位即可，与 shell 实际查找的语义一致；本项目仅 Linux 运行，无需跨平台抽象。
- **`metadata()` 失败静默跳过**：覆盖目录不存在、权限不足、符号链接断裂等场景；与 bash 实际行为一致。
- **每次调用现取 PATH 而非缓存**：后续若实现 `export` 修改环境变量，type 行为能立即反映最新 PATH，符合 shell 语义。
- **取首个命中即返回**：与 POSIX shell 行为一致（PATH 顺序优先）。
- **辅助函数返回 `PathBuf` 而非 `String`**：保留 `Path` 语义；输出时用 `.display()`，对非 UTF-8 路径降级显示而非 panic。
- **不抽模块**：单文件代码量仍可控（<100 行），辅助函数与 main 同文件即可；待后续阶段引入外部命令执行 + 更多内建后再考虑拆 `builtins.rs` / `path.rs`。

## 实施注意事项

- **输出格式严格匹配**：
  - 命中可执行：`{target} is {path}`（注意是 `is` 不是 `:`，与 builtin 同样使用 `is`）
  - 未命中：`{target}: not found`（冒号 + 空格）
- **不污染 stderr**：所有 type 输出走 stdout；目录读取错误静默吞掉，不要 eprintln。
- **目标 token 即查询对象**：使用 `target` 单 token（来自 `parts.next()`），不要使用 `line`，避免把多余参数也回显。
- **保留既有 BUILTINS 单一数据源**：新增逻辑只读不写 BUILTINS；后续阶段（如 pwd/cd）追加内建时仅需更新此数组。
- **fallback 字符串不可与 type 的 `not found` 混淆**：未知命令分支仍用 `command not found`，type 分支用 `not found`，与上阶段一致。

## 架构设计

```
const BUILTINS = ["echo", "exit", "type"]

fn find_in_path(name) -> Option<PathBuf>:
    PATH = env::var_os("PATH")?
    for dir in env::split_paths(&PATH):
        candidate = dir.join(name)
        meta = candidate.metadata().ok()?  # 失败则 continue
        if !meta.is_file(): continue
        if meta.permissions().mode() & 0o111 != 0:
            return Some(candidate)
    None

loop:
  print "$ " + flush
  read_line / trim / split_whitespace
  match cmd:
    "exit"  -> process::exit(code)
    "echo"  -> println join args
    "type"  -> let t = next_arg
               if BUILTINS.contains(t):   println!("{t} is a shell builtin")
               else if let Some(p) = find_in_path(t):                          # NEW
                   println!("{t} is {}", p.display())
               else:                       println!("{t}: not found")
    _       -> println!("{line}: command not found")
```

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 顶部追加 use：std::env / std::path::PathBuf / std::os::unix::fs::PermissionsExt
                  #          新增 fn find_in_path(name: &str) -> Option<PathBuf>
                  #          改造 "type" 分支：BUILTINS 优先 → find_in_path → not found
                  #          其他分支与控制流保持不变
```
