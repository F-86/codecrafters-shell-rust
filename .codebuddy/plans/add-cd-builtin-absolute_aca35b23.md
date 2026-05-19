---
name: add-cd-builtin-absolute
overview: "在现有 Rust shell 中新增 `cd` 内建命令（仅本阶段绝对路径分支）：调用 `std::env::set_current_dir()` 切换目录，失败时输出 `cd: <dir>: No such file or directory` 且保持当前目录不变；同步将 `cd` 注册到 `BUILTINS`。"
todos:
  - id: add-cd-builtin
    content: 在 src/main.rs 的 BUILTINS 追加 "cd"，并在 match cmd 新增 cd 分支调用 std::env::set_current_dir，失败时按格式打印错误
    status: completed
  - id: verify-build-and-test
    content: 运行 cargo build 与 ./your_program.sh，验证 cd 切换后 pwd 正确、无效路径报错且 cwd 不变、type cd 识别为 builtin
    status: completed
    dependencies:
      - add-cd-builtin
---

## 产品概述

在现有 Rust shell 项目中新增 `cd` 内建命令（本阶段仅处理绝对路径），用于切换 shell 进程的当前工作目录。

## 核心功能

- 用户输入 `cd <绝对路径>`：若目标目录存在且可进入，切换 shell 当前工作目录
- 目标目录不存在/不可访问时，输出 `cd: <directory>: No such file or directory`，且当前工作目录保持不变
- `cd` 注册为内建命令，使 `type cd` 输出 `cd is a shell builtin`
- 后续 `pwd` 调用应反映 `cd` 切换后的新目录

## 技术栈

- 语言：Rust（edition 2024，rust-version 1.95），沿用现有项目栈，零新增依赖
- 标准库 API：`std::env::set_current_dir(path)`，返回 `io::Result<()>`，失败时进程 cwd 自动保持不变（由 OS `chdir(2)` 语义保证）

## 实现思路

在 `src/main.rs` 现有 `match cmd` 分发块中追加 `"cd"` 分支：取首个参数作为目标路径，调用 `std::env::set_current_dir`，成功不输出，失败按测试要求格式打印错误。同步将 `"cd"` 加入 `BUILTINS` 常量数组，使 `type cd` 自动识别。

### 关键决策

- **直接调用 `set_current_dir` 而非先 `metadata` 检查再切换**：避免 TOCTOU（检查与使用之间的竞态），且 OS `chdir` 在失败时不会修改进程 cwd，天然满足"失败不改变 cwd"约束；单次系统调用比"stat + chdir"更高效，且能统一处理"不存在/非目录/无权限"等多种失败场景。
- **错误信息统一为 `cd: <dir>: No such file or directory`**：codecrafters 当前阶段测试只校验该字符串。真实 bash 会区分 `Not a directory`/`Permission denied` 等，但本阶段不要求，YAGNI 原则下采用统一信息以最小化改动并通过测试。
- **本阶段仅处理绝对路径**：`set_current_dir` 本身对相对路径、`~` 等也能工作（相对路径会基于当前 cwd 解析，`~` 不会被自动展开），但题面明确"focus on absolute paths"，相对路径与 `~` 留给后续阶段处理；当前实现不主动判别 `path.starts_with('/')`，让 `set_current_dir` 自然处理即可，不增加未要求的分支以避免过度设计。
- **错误输出通道**：参考现有 `command not found`、`pwd` 等已有分支均使用 `println!`/`eprintln!` 后继续 REPL；本实现使用 `println!` 与 `not found` 风格保持一致，便于测试用例稳定捕获（codecrafters 测试通常合并 stdout/stderr，但 stdout 更稳）。
- **无参数处理**：本阶段题面与测试均不覆盖 `cd` 无参数场景，简化处理为静默忽略（不调用 `set_current_dir`，避免误切换至 `~`），等后续 `~` 阶段再补全语义。

### 性能与影响

- 单次 `chdir(2)` 系统调用，O(1)，无热点
- 仅追加一个 match 分支与一个数组元素，零回归风险，向后兼容
- `pwd` 分支无需修改：其 `current_dir()` 会自动反映新 cwd

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] (1) BUILTINS 数组追加 "cd"；(2) match cmd 分发块新增 "cd" 分支：取 parts.next() 作为目标路径，调用 std::env::set_current_dir(target)，成功无输出，失败 println!("cd: {}: No such file or directory", target)；保持中文注释风格，REPL 不中断。
```