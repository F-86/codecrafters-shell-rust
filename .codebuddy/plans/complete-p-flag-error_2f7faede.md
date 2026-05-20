---
name: complete-p-flag-error
overview: "为 `complete` builtin 实现 `-p` 标志：本阶段不做规格存储，统一对任意命令名输出 `complete: <name>: no completion specification` 到 err_sink。"
todos:
  - id: impl-run-complete
    content: 在 src/builtins.rs 新增 run_complete，识别 -p <name> 并向 err_sink 输出 no completion specification 错误
    status: completed
  - id: wire-dispatch
    content: 在 src/main.rs use 列表与 dispatch match 追加 complete 分支，调用 run_complete 并兜底 write error
    status: completed
    dependencies:
      - impl-run-complete
  - id: verify-behavior
    content: cargo build 后非交互验证：complete -p foo 输出错误、type complete 仍为 builtin、其他 builtin 无回归
    status: completed
    dependencies:
      - wire-dispatch
---

## Product Overview

为 CodeCrafters Rust Shell 的 `complete` 内建命令增加 `-p` 标志识别能力，本阶段仅输出"未注册规格"的错误信息，不实际存储或打印任何规格。

## Core Features

- 识别 `complete -p <command>` 形态，向 stderr 输出 `complete: <command>: no completion specification`
- 错误信息支持 `2>` 重定向（与现有 builtin 错误处理范式一致）
- 不影响 `type complete` 仍输出 `complete is a shell builtin`
- 边界场景（`complete` 无参、`complete -p` 缺命令名、其他 flag）静默处理，与 `run_type` 风格保持一致

## 技术栈

沿用现有项目：Rust + 标准库，无新增依赖；不修改 `Cargo.toml`，不新建文件。

## 实现策略

**两点改动**，完全复用既有 builtin 处理范式：

1. **`src/builtins.rs`**：新增 `run_complete(err_sink: &mut dyn Write, args: &[String]) -> io::Result<()>`，签名风格与 `run_type` 对齐（错误统一写 err_sink，返回 `io::Result<()>`）。
2. **`src/main.rs:108` dispatch**：在 `"type"` 分支后追加 `"complete"` 分支，调用 `run_complete(&mut *err_sink, args)`，错误兜底走与 `run_type` 一致的 `shell: write error: {}` 输出，避免 `complete` 落入外部命令查找路径（否则 `find_in_path` 可能命中系统 bash 的 complete-like 二进制或留下残影）。

## 关键设计决策

### 解析规则

- `args` 为 `&[String]`：对应 `complete -p <name>` 时 `args[0]="-p"`、`args[1]="<name>"`。
- 仅匹配 `args.first() == Some("-p")` 且 `args.get(1)` 存在的精确二元形态：
- 命中 → 写 `complete: {name}: no completion specification\n` 到 err_sink。
- 不命中 → 静默 `Ok(())` 返回（参考 `run_type` line 106-108 的 `let Some(target) = args.first() else { return Ok(()); };` 风格）。

理由：本阶段题目仅要求 `-p <command>` 形态返回固定错误，**不要求**对其他 flag/缺参输出错误。静默返回避免污染 codecrafters 后续阶段的预期输出，是最稳妥的最小变更。

### 错误信道选择

- 走 `err_sink`（stderr）：与 bash 真实行为一致；与项目内 `run_type` 的 `not found`、`run_cd` 的 `No such file or directory` 等所有现存 builtin 错误信道完全一致；天然支持 `2>` 重定向，零额外代码。
- 题目测试一般同时捕获 stdout/stderr 校验，走 stderr 不会丢分。

### 不做之事（避免过度设计）

- 不为 `-p` 引入新的解析模块或状态机：单一 flag、固定 arity，inline 判定即可。
- 不预先设计规格存储结构（HashMap/Trie 等）：YAGNI，等下一阶段真正落地 `complete -C` 注册时再按届时需求选型。
- 不修改 `completion.rs`：上一阶段 BUILTINS 已含 `"complete"`，TAB 补全已自动覆盖。

## 实施细节（Implementation Notes）

### 输出格式精确性

- 严格按 `complete: {cmd}: no completion specification\n` 输出，使用 `writeln!` 自动追加 `\n`。
- 命令名直接回显用户传入的原始 `args[1]`，不做转义/裁剪（与题目"command name in the output matches the one passed to -p"匹配）。

### 复杂度与回归

- 时间复杂度：O(1)；空间复杂度：O(1)。无热路径性能顾虑。
- 兼容性：仅追加新 dispatch 分支与新函数，不动既有 builtin/外部命令路径，零回归风险。
- 上阶段 `type complete` 行为保持不变（BUILTINS 中 `"complete"` 已存在）。

## 目录结构

```
codecrafters-shell-rust/
└── src/
    ├── builtins.rs   # [MODIFY] 新增 run_complete(err_sink, args) -> io::Result<()>。
    │                 # 仅当 args == ["-p", <name>] 形态时向 err_sink 写
    │                 # "complete: <name>: no completion specification"；其他形态静默 Ok(())。
    │                 # 函数签名、错误信道、风格与同文件 run_type / run_cd 完全对齐。
    └── main.rs       # [MODIFY] 第 9 行 use 列表追加 run_complete；
                      # 第 108 行 dispatch match 在 "type" 分支后追加 "complete" 分支，
                      # 错误兜底使用与 "type" 分支一致的 "shell: write error: {}" eprintln 模式。
```

## 关键代码结构

```rust
// src/builtins.rs 新增函数签名（仅契约，不含完整实现）
pub fn run_complete(err_sink: &mut dyn Write, args: &[String]) -> io::Result<()>;
```