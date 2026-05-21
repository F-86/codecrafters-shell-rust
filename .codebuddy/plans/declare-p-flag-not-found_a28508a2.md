---
name: declare-p-flag-not-found
overview: 实现 declare -p NAME 不存在时的报错分支
todos:
  - id: add-run-declare
    content: 在 src/builtins.rs 新增 run_declare 函数，仅 -p NAME 分支写 err_sink，其它静默 Ok
    status: completed
  - id: wire-dispatch
    content: 在 src/main.rs 替换 "declare" => {} 占位 arm 为 run_declare 调用，更新注释保留关键约束
    status: completed
    dependencies:
      - add-run-declare
  - id: add-unit-tests
    content: 在 src/builtins.rs mod tests 补 3 个 declare 单测覆盖核心断言、硬编码契约、静默路径
    status: completed
    dependencies:
      - add-run-declare
  - id: verify-build-and-behavior
    content: 运行 cargo build / cargo test 验证零警告零回归并端到端跑 ./your_program.sh 验证 declare -p 错误输出
    status: completed
    dependencies:
      - wire-dispatch
      - add-unit-tests
---

## 产品概述

为 Rust 实现的 shell 在 declare 内建命令上扩展 `-p` 标志的「变量不存在」错误分支。本阶段无变量存储后端，硬编码视作所有 NAME 都不存在；任何 `declare -p NAME` 调用一律向 stderr 打印 `declare: NAME: not found`。后续阶段才会接入变量存储与命中分支。

## 核心功能

- **`-p NAME` 不存在分支**：`declare -p missing_variable` 向 stderr 输出 `declare: missing_variable: not found`（含末尾换行），stdout 无输出。
- **回显契约**：错误信息中的 NAME 直接来自 args 原文回显，不做合法标识符校验。
- **静默兜底**：`declare`（无参）、`declare -p`（缺 NAME）、`declare var=value` 等其它形态保持静默 `Ok(())`，不污染 stdout/stderr，也不走 `run_external` 兜底。
- **既有内建零回归**：`echo` / `exit` / `type` / `pwd` / `cd` / `complete` / `jobs` / `history` 行为完全不变；177 个既有测试持续通过。

## 技术栈选型

沿用现有项目栈，**不引入任何新依赖**：

- Rust edition 2024 / rustc ≥ 1.95
- 既有依赖：`anyhow` / `thiserror` / `bytes` / `rustyline` 均无需触碰

## 实施策略

### 高层方案

复刻项目此前 `run_complete` / `run_history` 等 builtin 首次接入时的同款做法，三点最小改动：

1. **在 `src/builtins.rs` 新增 `run_declare` 函数**：sink + err_sink + args 三参签名（与 `run_history` / `run_complete` 对齐），仅处理 `args == ["-p", NAME, ...]` 一条主路径写错误到 err_sink，其它形态一律 `Ok(())` 静默。
2. **在 `src/main.rs` 把现有 `"declare" => {}` 占位 arm 替换为 `run_declare` 调用**：沿用第 491–493 行 `run_history` 的 `if let Err(e) = ... { eprintln!("shell: write error: {}", e); }` 模板，IO 错误兜底语义一致。
3. **在 `src/builtins.rs` `#[cfg(test)] mod tests` 新增 3 个 declare 单测**：仿照 `complete_r_*` 的 `invoke` 薄封装风格，覆盖核心断言、硬编码契约、非 `-p` 静默路径。

### 关键技术决策与权衡

- **为何不实现存储后端**：题面 Notes 明确「hardcode the output for this stage」。预先做存储会引入未使用的 `Rc<RefCell<HashMap<String, String>>>` 状态、违反 YAGNI；本阶段直接「视作所有 NAME 不存在」最贴合题面字面契约。
- **为何 `sink` 仍保留在签名里**：后续阶段实现 `declare -p var` 命中分支需要写 stdout（输出 `declare -- var="value"`），保留 sink 参数避免后续阶段动调用点签名。本阶段在函数体里通过 `_ = sink;` 或干脆不消费来抑制 unused 警告（dyn Write 引用作为参数本身不会触发 unused 警告，与 `run_complete` 签名风格一致，无需额外标记）。
- **为何非 `-p` 路径全部静默 Ok**：第一阶段占位 arm 注释已明确强调「不能让 declare 调用走 run_external 报 command not found，违反 type 声称契约」。本阶段把空块换成函数调用，但非 `-p NAME` 路径的「静默 Ok」语义必须延续——不打印任何字节、不返回错误。
- **为何不校验 NAME 合法标识符**：bash 对 invalid identifier 走另一条 `not a valid identifier` 分支，本阶段题面只规定 `not found` 一种错误形式。统一按 not-found 处理与题面字面契约一致，且 tester 用例 `missing_variable` 本身合法。
- **错误格式契约**：`writeln!(err_sink, "declare: {}: not found", name)`，writeln 自带末尾 `\n`，与既有 `run_type` / `run_cd` / `run_complete` / `run_history` 全部对齐。

### 执行注记（grounded in exploration）

- **复用错误风格**：所有错误统一 `writeln!(err_sink, "declare: {}: not found", name)`，与项目内 5 处既有同款示例（`run_type` 第 178、`run_cd` 第 201、`run_complete` 第 257、`run_history` 第 553）零差异。
- **复用 dispatch 错误兜底**：`if let Err(e) = run_declare(...) { eprintln!("shell: write error: {}", e); }` 与 `run_history` 调用模板一字不差对齐。
- **保留第一阶段关键约束注释**：「必须位于 `_ => run_external` 之前」这条约束必须保留在新注释中——它是 declare 调用不掉到外部命令兜底的最后一道防线，删掉会埋下后续阶段误删 arm 时的回归风险。
- **Blast radius 控制**：仅 2 个文件改动（`src/builtins.rs` + `src/main.rs`），新增约 25 行代码（含函数 + 注释 + 3 个单测），零既有行删除/重排（注释更新除外），无 API 变更，编译期与运行期开销可忽略（dispatch 表多走一个分支跳转）。
- **零日志/无副作用**：本阶段不引入任何持久化、不读写文件、不向 stdout 写字节、不影响 PATH / env 状态。

## 架构设计

本阶段属于既有 builtin 模式的小规模扩展，无新架构组件。沿用现有「`builtins.rs` 提供 runner，`main.rs` dispatch 调用」单层结构，与 `run_pwd` / `run_type` / `run_cd` / `run_complete` / `run_history` 等同构。

数据流：

1. REPL 解析得到 `cmd = "declare"` + `args` slice
2. `main.rs` dispatch arm 命中 `"declare"`，传入 sink / err_sink / args 调 `run_declare`
3. `run_declare` 内部 match `args[0..2]`：`["-p", NAME, ...]` → 写 err_sink；其它 → `Ok(())`
4. dispatch 用 `if let Err` 包裹 IO 错误打印到 process stderr（与 `run_history` 同款）

## 目录结构

```
codecrafters-shell-rust/
├── src/
│   ├── builtins.rs   # [MODIFY] 1) 新增 pub fn run_declare(sink, err_sink, args) -> io::Result<()>，
│   │                 #             仅在 args[0]="-p" 且存在 args[1] 时 writeln!(err_sink,
│   │                 #             "declare: {}: not found", name) 返回结果；其它形态返回 Ok(())。
│   │                 #             函数文档注释说明本阶段「无存储后端、视作所有 NAME 不存在」契约，
│   │                 #             并预告后续阶段把 not-found 分支改为「先查存储、未命中再报错」即可。
│   │                 #          2) 在 #[cfg(test)] mod tests 末尾新增 3 个用例：
│   │                 #             - declare_p_missing_variable_writes_stderr（题面核心断言）
│   │                 #             - declare_p_any_name_treated_as_missing（验证硬编码视作不存在契约）
│   │                 #             - declare_silent_paths_no_output（空 args / 仅 -p / foo=bar）
│   │                 #             仿 complete_r_* 三用例 invoke 薄封装，构造 Vec<u8> sink+err、
│   │                 #             from_utf8 取出后断言。
│   └── main.rs       # [MODIFY] 1) 顶部 use builtins::{... run_declare} 追加 run_declare 导入。
│                     #          2) 第 495–506 行占位 arm 替换：
│                     #             - 注释顶部从「Stage 注册」更新为「Stage declare -p flag not-found branch」
│                     #             - 保留「必须位于 _ => run_external 之前」关键约束注释
│                     #             - arm 主体改为 if let Err(e) = run_declare(&mut *sink, &mut *err_sink, args)
│                     #               { eprintln!("shell: write error: {}", e); }，与第 491–493 行
│                     #               run_history 调用模板一字不差对齐。
```

**不改动**：`src/builtins.rs` BUILTINS 数组（第一阶段已含 declare）、`src/completion.rs`（遍历 BUILTINS 自动生效）、`src/exec.rs`（run_external 与 declare 互斥）、`src/redirect.rs`、`src/parser/*`、`tests/*`、`Cargo.toml`、`Cargo.lock`。

## Key Code Structure

```rust
/// `declare` 内建：本阶段仅实现 `-p NAME` 的「变量不存在」分支。
/// 题面 Notes：「hardcode the output」——尚未引入变量存储后端，所有 NAME 视作不存在。
/// 后续阶段把 not-found 分支替换为「先查存储、未命中再报错」即可。
pub fn run_declare(
    sink: &mut dyn Write,        // 占位：本阶段不写 stdout，为后续 -p 命中分支预留
    err_sink: &mut dyn Write,
    args: &[String],
) -> io::Result<()>;
```

## Agent Extensions

本阶段改动小、范围已精确定位，无需调用任何 Skill / MCP / SubAgent。