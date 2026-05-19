---
name: add-pwd-builtin
overview: 在现有 Rust shell 中新增 `pwd` 内建命令：通过 `std::env::current_dir()` 获取当前工作目录绝对路径并打印，同时将 `pwd` 注册到 `BUILTINS` 列表使 `type pwd` 正确识别。
todos:
  - id: add-pwd-builtin
    content: 在 src/main.rs 的 BUILTINS 数组追加 "pwd"，并在 match cmd 中新增 pwd 分支调用 std::env::current_dir() 打印绝对路径
    status: completed
  - id: verify-build-and-test
    content: 本地运行 cargo build 与 ./your_program.sh，验证 pwd 输出正确路径且 type pwd 识别为 builtin
    status: completed
    dependencies:
      - add-pwd-builtin
---

## 产品概述

在现有 Rust shell 项目中新增 `pwd` 内建命令，用于打印 shell 当前工作目录的完整绝对路径。

## 核心功能

- 用户在 REPL 中输入 `pwd` 后，shell 输出当前进程的工作目录绝对路径并换行
- `pwd` 作为内建命令注册，使 `type pwd` 输出 `pwd is a shell builtin`
- 多余参数被静默忽略（与 POSIX 行为一致）
- 获取当前工作目录失败时，将错误信息输出到 stderr，但不中断 REPL 循环

## 技术栈

- 语言：Rust（edition 2024，rust-version 1.95），沿用现有项目栈
- 标准库 API：`std::env::current_dir()`（返回 `io::Result<PathBuf>`，OS 保证为绝对路径）
- 输出：`println!` + `path.display()`，错误用 `eprintln!`

## 实现思路

在 `src/main.rs` 现有 `match cmd` 内建命令分发结构中追加 `"pwd"` 分支，调用 `std::env::current_dir()` 获取路径并打印；同步将 `"pwd"` 加入 `BUILTINS` 常量数组，使 `type pwd` 自动识别为 builtin（无需修改 `type` 分支逻辑）。

### 关键决策

- **使用 `std::env::current_dir()` 而非 `$PWD` 环境变量**：标准库直接调用 `getcwd(2)` 返回真实工作目录，结果可靠且始终为绝对路径，避免 `$PWD` 可能未设置或被污染的问题；本阶段尚未实现 `cd`，无需考虑符号链接相关的逻辑路径差异。
- **复用 `BUILTINS` 注册机制**：`type` 分支已通过 `BUILTINS.contains(&target)` 查询，加入数组即生效，无重复逻辑，符合 DRY 与代码注释中"后续阶段新增内建只需在此处追加"的设计预期。
- **参数处理**：忽略 `parts` 剩余 token，与多数 shell 对 `pwd` 的宽松处理一致；不引入 `-L`/`-P` 选项，避免超出当前阶段需求（YAGNI）。
- **错误处理**：`current_dir()` 失败（如目录被删除、权限问题）时通过 `eprintln!` 输出错误且不 break，与现有 REPL 内错误处理风格保持一致，避免误中断交互会话。

### 性能与影响

- 单次 `getcwd` 系统调用，O(1)，无热点路径影响
- 仅新增一个 match 分支与一个数组元素，零回归风险，向后兼容

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] (1) 在 BUILTINS 数组追加 "pwd"；(2) 在 match cmd 分发块中新增 "pwd" 分支：调用 std::env::current_dir()，成功则 println!("{}", path.display())，失败则 eprintln! 错误信息并继续 REPL；保持中文注释风格与现有错误处理一致。
```