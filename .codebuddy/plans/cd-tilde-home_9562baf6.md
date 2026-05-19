---
name: cd-tilde-home
overview: 在 `cd` 分支顶部加一段轻量预处理：当参数等于 `~` 时，用 `std::env::var("HOME")` 的值替换后再交给 `set_current_dir`；其余路径行为保持不变。
todos:
  - id: add-tilde-expansion
    content: 在 src/main.rs 的 cd 分支内，对 target == "~" 读取 HOME 后再调用 set_current_dir，HOME 缺失走错误格式
    status: completed
  - id: verify-tilde-cd
    content: 运行 cargo build 与 ./your_program.sh，验证 cd ~ 切换到 $HOME、回归绝对/相对路径与错误格式
    status: completed
    dependencies:
      - add-tilde-expansion
---

## 产品概述

扩展现有 Rust shell 的 `cd` 内建命令，新增对 `~` 字符的支持，使其能够展开为用户主目录（由 `HOME` 环境变量指定）。

## 核心功能

- 输入 `cd ~`：读取 `HOME` 环境变量并切换到该路径
- 切换成功无输出；如 `HOME` 未设置或目标不可达，按既有错误格式输出 `cd: ~: No such file or directory` 且当前工作目录保持不变
- 错误信息回显用户原始输入 `~`，不展开为绝对路径（与 bash 一致）
- 不影响既有的绝对路径、相对路径切换行为

## 技术栈

- 语言：Rust（edition 2024，rust-version 1.95），沿用现有项目栈，零新增依赖
- 标准库 API：
- `std::env::var("HOME")`：返回 `Result<String, VarError>`，对应题目提示的"读取 HOME 环境变量"
- `std::env::set_current_dir`：既有调用，封装 `chdir(2)`

## 实现思路

### 关键洞察

上一阶段 `cd` 分支（约 `src/main.rs` 第 88-100 行）已直接将 `target` 透传给 `set_current_dir`。本阶段只需在该分支内、调用 `set_current_dir` **之前**，对 `target == "~"` 做一次字符串展开，得到的真实路径再交给 `chdir(2)`。

### 关键决策

- **仅匹配精确 `~`**：本阶段题目 Tests 只验证 `cd ~`，不涉及 `~/subdir`。采用精确匹配 (`target == "~"`) 而非 `starts_with("~")`，避免误展开像 `~user` 这类 bash 中含义不同的形式（用户家目录而非 `$HOME`），保持最小语义、严格匹配测试，符合 YAGNI。
- **`HOME` 缺失的容错**：理论上测试器会保证 `HOME` 已设置；为避免 panic 导致 REPL 中断，`env::var("HOME")` 失败时按既有错误格式输出 `cd: ~: No such file or directory`，与失败语义统一。
- **错误信息回显原始输入**：参考 bash 行为，失败信息中仍显示 `~` 而非展开后的路径，便于用户理解输入。
- **不修改外层结构**：保留 `if let Some(target) = parts.next()` 的现有分支结构与"无参数静默跳过"的现有约定（`cd` 无参语义题目未要求，留待后续）。

### 性能与影响

- 单次环境变量读取（O(路径长度)）+ 单次 `chdir(2)`，无热点
- 仅扩展 `cd` 分支约 5-7 行代码，对其它内建/外部命令零影响，向后兼容

## 实现注意事项

- 复用现有 `cd` 分支结构与错误打印格式，不抽函数（一处使用，避免过度设计）
- 中文注释风格与现有代码保持一致
- 预留下一阶段扩展空间（如 `~/subdir`）：注释中说明本阶段只支持精确 `~`，但实现位置允许将来无缝替换为 `strip_prefix` 模式

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 修改 cd 分支（约 88-100 行）：在已有 if let Some(target) 内、set_current_dir 之前，新增 ~ 展开逻辑——若 target == "~"，读取 HOME 环境变量作为实际路径；HOME 缺失时按 cd: ~: No such file or directory 错误格式输出且不切换；其它路径形式（绝对/相对）走原有逻辑不变；错误信息统一回显用户原始输入 target。
```